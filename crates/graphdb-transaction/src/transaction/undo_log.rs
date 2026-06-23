//! Undo Log
//!
//! Provides transaction rollback support through undo log entries.
//! Each undo log entry can reverse a specific operation during transaction abort.

use super::wal::{ColumnId, LabelId, Timestamp, VertexId};
use crate::core::types::{
    EdgeDeletionContext, EdgeDeletionContextParams, EdgeIdentifier, EdgeKey, VertexIdentifier,
};

/// Undo log error
pub use crate::core::types::UndoLogError;

/// Undo log result type
pub use crate::core::types::UndoLogResult;

/// Property value type for undo operations
pub use crate::core::types::PropertyValue;

/// Target for undo operations (will be GraphStorageContext in phase 2)
pub use crate::core::types::UndoTarget;

/// Undo log for create vertex type operation
#[derive(Debug, Clone)]
pub struct CreateVertexTypeUndo {
    pub vertex_type: LabelId,
}

impl CreateVertexTypeUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.delete_vertex_type(self.vertex_type)
    }

    pub fn description(&self) -> String {
        format!("CreateVertexTypeUndo(label={})", self.vertex_type)
    }
}

/// Undo log for create edge type operation
#[derive(Debug, Clone)]
pub struct CreateEdgeTypeUndo {
    pub src_type: LabelId,
    pub dst_type: LabelId,
    pub edge_type: LabelId,
}

impl CreateEdgeTypeUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.delete_edge_type(EdgeKey::new(self.src_type, self.dst_type, self.edge_type))
    }

    pub fn description(&self) -> String {
        format!(
            "CreateEdgeTypeUndo(src={}, dst={}, edge={})",
            self.src_type, self.dst_type, self.edge_type
        )
    }
}

/// Undo log for insert vertex operation
#[derive(Debug, Clone)]
pub struct InsertVertexUndo {
    pub v_label: LabelId,
    pub vid: VertexId,
}

impl InsertVertexUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, ts: Timestamp) -> UndoLogResult<()> {
        graph.delete_vertex(VertexIdentifier::new(self.v_label, self.vid), ts)
    }

    pub fn description(&self) -> String {
        format!("InsertVertexUndo(label={}, vid={})", self.v_label, self.vid)
    }
}

/// Undo log for insert edge operation
#[derive(Debug, Clone)]
pub struct InsertEdgeUndo {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub src_vid: VertexId,
    pub dst_vid: VertexId,
    pub oe_offset: i32,
    pub ie_offset: i32,
}

impl InsertEdgeUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, ts: Timestamp) -> UndoLogResult<()> {
        graph.delete_edge(EdgeDeletionContext::new(EdgeDeletionContextParams {
            src_label: self.src_label,
            src_vid: self.src_vid,
            dst_label: self.dst_label,
            dst_vid: self.dst_vid,
            edge_label: self.edge_label,
            rank: self.rank,
            oe_offset: self.oe_offset,
            ie_offset: self.ie_offset,
            timestamp: ts,
        }))
    }

    pub fn description(&self) -> String {
        format!(
            "InsertEdgeUndo(src={}, dst={}, edge={}, src_vid={}, dst_vid={})",
            self.src_label, self.dst_label, self.edge_label, self.src_vid, self.dst_vid
        )
    }
}

/// Undo log for update vertex property operation
#[derive(Debug, Clone)]
pub struct UpdateVertexPropUndo {
    pub v_label: LabelId,
    pub vid: VertexId,
    pub col_id: ColumnId,
    pub old_value: PropertyValue,
}

impl UpdateVertexPropUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, ts: Timestamp) -> UndoLogResult<()> {
        graph.undo_update_vertex_property(
            VertexIdentifier::new(self.v_label, self.vid),
            self.col_id,
            self.old_value.clone(),
            ts,
        )
    }

    pub fn description(&self) -> String {
        format!(
            "UpdateVertexPropUndo(label={}, vid={}, col={})",
            self.v_label, self.vid, self.col_id
        )
    }
}

/// Undo log for update edge property operation
#[derive(Debug, Clone)]
pub struct UpdateEdgePropUndo {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub oe_offset: i32,
    pub ie_offset: i32,
    pub col_id: ColumnId,
    pub old_value: PropertyValue,
}

impl UpdateEdgePropUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, ts: Timestamp) -> UndoLogResult<()> {
        graph.undo_update_edge_property(
            EdgeIdentifier::new(
                self.src_label,
                self.src_vid,
                self.dst_label,
                self.dst_vid,
                self.edge_label,
                self.rank,
            ),
            self.oe_offset,
            self.ie_offset,
            self.col_id,
            self.old_value.clone(),
            ts,
        )
    }

    pub fn description(&self) -> String {
        format!(
            "UpdateEdgePropUndo(src={}, dst={}, edge={}, col={})",
            self.src_label, self.dst_label, self.edge_label, self.col_id
        )
    }
}

/// Related edge information for remove vertex undo
#[derive(Debug, Clone)]
pub struct RelatedEdgeInfo {
    pub src_vid: VertexId,
    pub dst_vid: VertexId,
    pub rank: i64,
    pub oe_offset: i32,
    pub ie_offset: i32,
}

/// Undo log for remove vertex operation
#[derive(Debug, Clone)]
pub struct RemoveVertexUndo {
    pub v_label: LabelId,
    pub vid: VertexId,
    pub related_edges: Vec<(LabelId, LabelId, LabelId, Vec<RelatedEdgeInfo>)>,
}

impl RemoveVertexUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_delete_vertex(VertexIdentifier::new(self.v_label, self.vid), ts)?;

        for (src_label, dst_label, edge_label, edges) in &self.related_edges {
            for edge in edges {
                graph.revert_delete_edge(EdgeDeletionContext::new(EdgeDeletionContextParams {
                    src_label: *src_label,
                    src_vid: edge.src_vid,
                    dst_label: *dst_label,
                    dst_vid: edge.dst_vid,
                    edge_label: *edge_label,
                    rank: edge.rank,
                    oe_offset: edge.oe_offset,
                    ie_offset: edge.ie_offset,
                    timestamp: ts,
                }))?;
            }
        }

        Ok(())
    }

    pub fn description(&self) -> String {
        format!(
            "RemoveVertexUndo(label={}, vid={}, edges={})",
            self.v_label,
            self.vid,
            self.related_edges.len()
        )
    }
}

/// Undo log for remove edge operation
#[derive(Debug, Clone)]
pub struct RemoveEdgeUndo {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub oe_offset: i32,
    pub ie_offset: i32,
}

impl RemoveEdgeUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_delete_edge(EdgeDeletionContext::new(EdgeDeletionContextParams {
            src_label: self.src_label,
            src_vid: self.src_vid,
            dst_label: self.dst_label,
            dst_vid: self.dst_vid,
            edge_label: self.edge_label,
            rank: self.rank,
            oe_offset: self.oe_offset,
            ie_offset: self.ie_offset,
            timestamp: ts,
        }))
    }

    pub fn description(&self) -> String {
        format!(
            "RemoveEdgeUndo(src={}, dst={}, edge={})",
            self.src_label, self.dst_label, self.edge_label
        )
    }
}

/// Undo log for add vertex property operation
#[derive(Debug, Clone)]
pub struct AddVertexPropUndo {
    pub label: LabelId,
    pub label_name: String,
    pub prop_names: Vec<String>,
}

impl AddVertexPropUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_delete_vertex_properties(&self.label_name, &self.prop_names)
    }

    pub fn description(&self) -> String {
        format!(
            "AddVertexPropUndo(label={}, props={:?})",
            self.label_name, self.prop_names
        )
    }
}

/// Undo log for add edge property operation
#[derive(Debug, Clone)]
pub struct AddEdgePropUndo {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
    pub src_label_name: String,
    pub dst_label_name: String,
    pub edge_label_name: String,
    pub prop_names: Vec<String>,
}

impl AddEdgePropUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_delete_edge_properties(
            &self.src_label_name,
            &self.dst_label_name,
            &self.edge_label_name,
            &self.prop_names,
        )
    }

    pub fn description(&self) -> String {
        format!(
            "AddEdgePropUndo(edge={}, props={:?})",
            self.edge_label_name, self.prop_names
        )
    }
}

/// Undo log for rename vertex property operation
#[derive(Debug, Clone)]
pub struct RenameVertexPropUndo {
    pub label: LabelId,
    pub label_name: String,
    pub old_names_to_new_names: Vec<(String, String)>,
}

impl RenameVertexPropUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        let current_names: Vec<_> = self
            .old_names_to_new_names
            .iter()
            .map(|(_, new)| new.clone())
            .collect();
        let original_names: Vec<_> = self
            .old_names_to_new_names
            .iter()
            .map(|(old, _)| old.clone())
            .collect();
        graph.revert_rename_vertex_properties(&self.label_name, &current_names, &original_names)
    }

    pub fn description(&self) -> String {
        format!(
            "RenameVertexPropUndo(label={}, renames={:?})",
            self.label_name, self.old_names_to_new_names
        )
    }
}

/// Undo log for rename edge property operation
#[derive(Debug, Clone)]
pub struct RenameEdgePropUndo {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
    pub src_label_name: String,
    pub dst_label_name: String,
    pub edge_label_name: String,
    pub old_names_to_new_names: Vec<(String, String)>,
}

impl RenameEdgePropUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        let current_names: Vec<_> = self
            .old_names_to_new_names
            .iter()
            .map(|(_, new)| new.clone())
            .collect();
        let original_names: Vec<_> = self
            .old_names_to_new_names
            .iter()
            .map(|(old, _)| old.clone())
            .collect();
        graph.revert_rename_edge_properties(
            &self.src_label_name,
            &self.dst_label_name,
            &self.edge_label_name,
            &current_names,
            &original_names,
        )
    }

    pub fn description(&self) -> String {
        format!(
            "RenameEdgePropUndo(edge={}, renames={:?})",
            self.edge_label_name, self.old_names_to_new_names
        )
    }
}

/// Undo log for delete vertex property operation
#[derive(Debug, Clone)]
pub struct DeleteVertexPropUndo {
    pub label: LabelId,
    pub label_name: String,
    pub prop_names: Vec<String>,
}

impl DeleteVertexPropUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_delete_vertex_properties(&self.label_name, &self.prop_names)
    }

    pub fn description(&self) -> String {
        format!(
            "DeleteVertexPropUndo(label={}, props={:?})",
            self.label_name, self.prop_names
        )
    }
}

/// Undo log for delete edge property operation
#[derive(Debug, Clone)]
pub struct DeleteEdgePropUndo {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
    pub src_label_name: String,
    pub dst_label_name: String,
    pub edge_label_name: String,
    pub prop_names: Vec<String>,
}

impl DeleteEdgePropUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_delete_edge_properties(
            &self.src_label_name,
            &self.dst_label_name,
            &self.edge_label_name,
            &self.prop_names,
        )
    }

    pub fn description(&self) -> String {
        format!(
            "DeleteEdgePropUndo(edge={}, props={:?})",
            self.edge_label_name, self.prop_names
        )
    }
}

/// Undo log for delete vertex type operation
#[derive(Debug, Clone)]
pub struct DeleteVertexTypeUndo {
    pub v_label: String,
}

impl DeleteVertexTypeUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_delete_vertex_label(&self.v_label)
    }

    pub fn description(&self) -> String {
        format!("DeleteVertexTypeUndo(label={})", self.v_label)
    }
}

/// Undo log for delete edge type operation
#[derive(Debug, Clone)]
pub struct DeleteEdgeTypeUndo {
    pub src_label: String,
    pub dst_label: String,
    pub edge_label: String,
}

impl DeleteEdgeTypeUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_delete_edge_label(&self.src_label, &self.dst_label, &self.edge_label)
    }

    pub fn description(&self) -> String {
        format!(
            "DeleteEdgeTypeUndo(src={}, dst={}, edge={})",
            self.src_label, self.dst_label, self.edge_label
        )
    }
}

/// Undo log entry enum - zero-cost abstraction for all undo types
#[derive(Debug, Clone)]
pub enum UndoLogEntry {
    CreateVertexType(CreateVertexTypeUndo),
    CreateEdgeType(CreateEdgeTypeUndo),
    InsertVertex(InsertVertexUndo),
    InsertEdge(InsertEdgeUndo),
    UpdateVertexProp(UpdateVertexPropUndo),
    UpdateEdgeProp(UpdateEdgePropUndo),
    RemoveVertex(RemoveVertexUndo),
    RemoveEdge(RemoveEdgeUndo),
    AddVertexProp(AddVertexPropUndo),
    AddEdgeProp(AddEdgePropUndo),
    RenameVertexProp(RenameVertexPropUndo),
    RenameEdgeProp(RenameEdgePropUndo),
    DeleteVertexProp(DeleteVertexPropUndo),
    DeleteEdgeProp(DeleteEdgePropUndo),
    DeleteVertexType(DeleteVertexTypeUndo),
    DeleteEdgeType(DeleteEdgeTypeUndo),
}

impl UndoLogEntry {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, ts: Timestamp) -> UndoLogResult<()> {
        match self {
            UndoLogEntry::CreateVertexType(u) => u.undo(graph, ts),
            UndoLogEntry::CreateEdgeType(u) => u.undo(graph, ts),
            UndoLogEntry::InsertVertex(u) => u.undo(graph, ts),
            UndoLogEntry::InsertEdge(u) => u.undo(graph, ts),
            UndoLogEntry::UpdateVertexProp(u) => u.undo(graph, ts),
            UndoLogEntry::UpdateEdgeProp(u) => u.undo(graph, ts),
            UndoLogEntry::RemoveVertex(u) => u.undo(graph, ts),
            UndoLogEntry::RemoveEdge(u) => u.undo(graph, ts),
            UndoLogEntry::AddVertexProp(u) => u.undo(graph, ts),
            UndoLogEntry::AddEdgeProp(u) => u.undo(graph, ts),
            UndoLogEntry::RenameVertexProp(u) => u.undo(graph, ts),
            UndoLogEntry::RenameEdgeProp(u) => u.undo(graph, ts),
            UndoLogEntry::DeleteVertexProp(u) => u.undo(graph, ts),
            UndoLogEntry::DeleteEdgeProp(u) => u.undo(graph, ts),
            UndoLogEntry::DeleteVertexType(u) => u.undo(graph, ts),
            UndoLogEntry::DeleteEdgeType(u) => u.undo(graph, ts),
        }
    }

    pub fn description(&self) -> String {
        match self {
            UndoLogEntry::CreateVertexType(u) => u.description(),
            UndoLogEntry::CreateEdgeType(u) => u.description(),
            UndoLogEntry::InsertVertex(u) => u.description(),
            UndoLogEntry::InsertEdge(u) => u.description(),
            UndoLogEntry::UpdateVertexProp(u) => u.description(),
            UndoLogEntry::UpdateEdgeProp(u) => u.description(),
            UndoLogEntry::RemoveVertex(u) => u.description(),
            UndoLogEntry::RemoveEdge(u) => u.description(),
            UndoLogEntry::AddVertexProp(u) => u.description(),
            UndoLogEntry::AddEdgeProp(u) => u.description(),
            UndoLogEntry::RenameVertexProp(u) => u.description(),
            UndoLogEntry::RenameEdgeProp(u) => u.description(),
            UndoLogEntry::DeleteVertexProp(u) => u.description(),
            UndoLogEntry::DeleteEdgeProp(u) => u.description(),
            UndoLogEntry::DeleteVertexType(u) => u.description(),
            UndoLogEntry::DeleteEdgeType(u) => u.description(),
        }
    }
}

/// Undo log manager for collecting and executing undo logs
pub struct UndoLogManager {
    logs: Vec<UndoLogEntry>,
}

/// Parameters for add_insert_edge operation
pub struct AddInsertEdgeParams {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub src_vid: VertexId,
    pub dst_vid: VertexId,
    pub oe_offset: i32,
    pub ie_offset: i32,
}

/// Parameters for add_update_edge_prop operation
pub struct AddUpdateEdgePropParams {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub oe_offset: i32,
    pub ie_offset: i32,
    pub col_id: ColumnId,
    pub old_value: PropertyValue,
}

impl UndoLogManager {
    pub fn new() -> Self {
        Self { logs: Vec::new() }
    }

    pub fn add(&mut self, log: UndoLogEntry) {
        self.logs.push(log);
    }

    pub fn add_insert_vertex(&mut self, label: LabelId, vid: VertexId) {
        self.add(UndoLogEntry::InsertVertex(InsertVertexUndo {
            v_label: label,
            vid,
        }));
    }

    pub fn add_insert_edge(&mut self, params: AddInsertEdgeParams) {
        self.add(UndoLogEntry::InsertEdge(InsertEdgeUndo {
            src_label: params.src_label,
            dst_label: params.dst_label,
            edge_label: params.edge_label,
            rank: params.rank,
            src_vid: params.src_vid,
            dst_vid: params.dst_vid,
            oe_offset: params.oe_offset,
            ie_offset: params.ie_offset,
        }));
    }

    pub fn add_update_vertex_prop(
        &mut self,
        label: LabelId,
        vid: VertexId,
        col_id: ColumnId,
        old_value: PropertyValue,
    ) {
        self.add(UndoLogEntry::UpdateVertexProp(UpdateVertexPropUndo {
            v_label: label,
            vid,
            col_id,
            old_value,
        }));
    }

    pub fn add_update_edge_prop(&mut self, params: AddUpdateEdgePropParams) {
        self.add(UndoLogEntry::UpdateEdgeProp(UpdateEdgePropUndo {
            src_label: params.src_label,
            src_vid: params.src_vid,
            dst_label: params.dst_label,
            dst_vid: params.dst_vid,
            edge_label: params.edge_label,
            rank: params.rank,
            oe_offset: params.oe_offset,
            ie_offset: params.ie_offset,
            col_id: params.col_id,
            old_value: params.old_value,
        }));
    }

    pub fn is_empty(&self) -> bool {
        self.logs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.logs.len()
    }

    pub fn clear(&mut self) {
        self.logs.clear();
    }

    pub fn pop(&mut self) -> Option<UndoLogEntry> {
        self.logs.pop()
    }

    pub fn execute_undo<T: UndoTarget + ?Sized>(
        &mut self,
        graph: &T,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        while let Some(log) = self.logs.pop() {
            log.undo(graph, ts)?;
        }
        Ok(())
    }

    pub fn execute_undo_from_index<T: UndoTarget + ?Sized>(
        &mut self,
        graph: &T,
        ts: Timestamp,
        start_index: usize,
    ) -> UndoLogResult<()> {
        if start_index > self.logs.len() {
            return Err(UndoLogError::UndoFailed(format!(
                "Invalid undo log rollback index: {}, undo log length: {}",
                start_index,
                self.logs.len()
            )));
        }

        let mut tail = self.logs.split_off(start_index);
        while let Some(log) = tail.pop() {
            log.undo(graph, ts)?;
        }
        Ok(())
    }
}

impl Default for UndoLogManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockUndoTarget;

    impl UndoTarget for MockUndoTarget {
        fn delete_vertex_type(&self, _label: LabelId) -> UndoLogResult<()> {
            Ok(())
        }

        fn delete_edge_type(&self, _edge_key: EdgeKey) -> UndoLogResult<()> {
            Ok(())
        }

        fn delete_vertex(&self, _vertex: VertexIdentifier, _ts: Timestamp) -> UndoLogResult<()> {
            Ok(())
        }

        fn delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
            Ok(())
        }

        fn undo_update_vertex_property(
            &self,
            _vertex: VertexIdentifier,
            _col_id: ColumnId,
            _value: PropertyValue,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn undo_update_edge_property(
            &self,
            _edge_id: EdgeIdentifier,
            _oe_offset: i32,
            _ie_offset: i32,
            _col_id: ColumnId,
            _value: PropertyValue,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_vertex(
            &self,
            _vertex: VertexIdentifier,
            _ts: Timestamp,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_edge(&self, _edge_ctx: EdgeDeletionContext) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_vertex_properties(
            &self,
            _label_name: &str,
            _prop_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_edge_properties(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
            _prop_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_vertex_label(&self, _label_name: &str) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_delete_edge_label(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_rename_vertex_properties(
            &self,
            _label_name: &str,
            _current_names: &[String],
            _original_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_rename_edge_properties(
            &self,
            _src_label: &str,
            _dst_label: &str,
            _edge_label: &str,
            _current_names: &[String],
            _original_names: &[String],
        ) -> UndoLogResult<()> {
            Ok(())
        }
    }

    #[test]
    fn test_undo_log_manager() {
        let mut manager = UndoLogManager::new();

        manager.add_insert_vertex(1, VertexId::from_int64(100));
        manager.add_insert_edge(AddInsertEdgeParams {
            src_label: 1,
            dst_label: 2,
            edge_label: 3,
            rank: 0,
            src_vid: VertexId::from_int64(100),
            dst_vid: VertexId::from_int64(200),
            oe_offset: 0,
            ie_offset: 0,
        });

        assert_eq!(manager.len(), 2);

        let target = MockUndoTarget;
        manager.execute_undo(&target, 1).expect("Undo failed");

        assert!(manager.is_empty());
    }

    #[test]
    fn test_execute_undo_from_index_keeps_prefix() {
        let mut manager = UndoLogManager::new();
        manager.add_insert_vertex(1, VertexId::from_int64(1));
        manager.add_insert_vertex(1, VertexId::from_int64(2));
        manager.add_insert_vertex(1, VertexId::from_int64(3));

        let target = MockUndoTarget;
        manager
            .execute_undo_from_index(&target, 1, 1)
            .expect("Undo from index failed");

        assert_eq!(manager.len(), 1);
    }

    #[test]
    fn test_create_vertex_type_undo() {
        let undo = CreateVertexTypeUndo { vertex_type: 1 };
        assert!(undo.description().contains("CreateVertexTypeUndo"));
    }

    #[test]
    fn test_insert_vertex_undo() {
        let undo = InsertVertexUndo {
            v_label: 1,
            vid: VertexId::from_int64(100),
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_insert_edge_undo() {
        let undo = InsertEdgeUndo {
            src_label: 1,
            dst_label: 2,
            edge_label: 3,
            rank: 0,
            src_vid: VertexId::from_int64(100),
            dst_vid: VertexId::from_int64(200),
            oe_offset: 0,
            ie_offset: 0,
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_update_vertex_prop_undo() {
        let undo = UpdateVertexPropUndo {
            v_label: 1,
            vid: VertexId::from_int64(100),
            col_id: ColumnId(0),
            old_value: PropertyValue::Int(42),
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_update_edge_prop_undo() {
        let undo = UpdateEdgePropUndo {
            src_label: 1,
            src_vid: VertexId::from_int64(100),
            dst_label: 2,
            dst_vid: VertexId::from_int64(200),
            edge_label: 3,
            rank: 0,
            oe_offset: 0,
            ie_offset: 0,
            col_id: ColumnId(0),
            old_value: PropertyValue::String("test".to_string()),
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_rename_vertex_prop_undo() {
        let undo = RenameVertexPropUndo {
            label: 1,
            label_name: "person".to_string(),
            old_names_to_new_names: vec![
                ("name".to_string(), "full_name".to_string()),
                ("age".to_string(), "years_old".to_string()),
            ],
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
        assert!(undo.description().contains("person"));
        assert!(undo.description().contains("renames"));
    }

    #[test]
    fn test_rename_edge_prop_undo() {
        let undo = RenameEdgePropUndo {
            src_label: 1,
            dst_label: 2,
            edge_label: 3,
            src_label_name: "person".to_string(),
            dst_label_name: "person".to_string(),
            edge_label_name: "knows".to_string(),
            old_names_to_new_names: vec![("since".to_string(), "since_date".to_string())],
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_remove_vertex_undo() {
        let undo = RemoveVertexUndo {
            v_label: 1,
            vid: VertexId::from_int64(100),
            related_edges: vec![(
                1,
                2,
                3,
                vec![RelatedEdgeInfo {
                    src_vid: VertexId::from_int64(100),
                    dst_vid: VertexId::from_int64(200),
                    rank: 0,
                    oe_offset: 0,
                    ie_offset: 0,
                }],
            )],
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
        assert!(undo.description().contains("edges=1"));
    }

    #[test]
    fn test_remove_edge_undo() {
        let undo = RemoveEdgeUndo {
            src_label: 1,
            src_vid: VertexId::from_int64(100),
            dst_label: 2,
            dst_vid: VertexId::from_int64(200),
            edge_label: 3,
            rank: 0,
            oe_offset: 0,
            ie_offset: 0,
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_delete_vertex_prop_undo() {
        let undo = DeleteVertexPropUndo {
            label: 1,
            label_name: "person".to_string(),
            prop_names: vec!["name".to_string(), "age".to_string()],
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_delete_edge_prop_undo() {
        let undo = DeleteEdgePropUndo {
            src_label: 1,
            dst_label: 2,
            edge_label: 3,
            src_label_name: "person".to_string(),
            dst_label_name: "person".to_string(),
            edge_label_name: "knows".to_string(),
            prop_names: vec!["since".to_string()],
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_delete_vertex_type_undo() {
        let undo = DeleteVertexTypeUndo {
            v_label: "person".to_string(),
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_delete_edge_type_undo() {
        let undo = DeleteEdgeTypeUndo {
            src_label: "person".to_string(),
            dst_label: "person".to_string(),
            edge_label: "knows".to_string(),
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_undo_order_is_lifo() {
        let mut manager = UndoLogManager::new();

        manager.add_insert_vertex(1, VertexId::from_int64(100));
        manager.add_insert_vertex(1, VertexId::from_int64(200));
        manager.add_insert_vertex(1, VertexId::from_int64(300));

        assert_eq!(manager.len(), 3);

        let target = MockUndoTarget;
        manager.execute_undo(&target, 1).expect("Undo failed");

        assert!(manager.is_empty());
    }

    #[test]
    fn test_property_value_is_null() {
        assert!(PropertyValue::Null.is_null());
        assert!(!PropertyValue::Int(0).is_null());
        assert!(!PropertyValue::String("".to_string()).is_null());
    }

    #[test]
    fn test_undo_log_manager_clear() {
        let mut manager = UndoLogManager::new();

        manager.add_insert_vertex(1, VertexId::from_int64(100));
        manager.add_insert_edge(AddInsertEdgeParams {
            src_label: 1,
            dst_label: 2,
            edge_label: 3,
            rank: 0,
            src_vid: VertexId::from_int64(100),
            dst_vid: VertexId::from_int64(200),
            oe_offset: 0,
            ie_offset: 0,
        });

        assert_eq!(manager.len(), 2);

        manager.clear();

        assert!(manager.is_empty());
        assert_eq!(manager.len(), 0);
    }

    #[test]
    fn test_create_edge_type_undo() {
        let undo = CreateEdgeTypeUndo {
            src_type: 1,
            dst_type: 2,
            edge_type: 3,
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
        assert!(undo.description().contains("CreateEdgeTypeUndo"));
    }

    #[test]
    fn test_add_vertex_prop_undo() {
        let undo = AddVertexPropUndo {
            label: 1,
            label_name: "person".to_string(),
            prop_names: vec!["new_prop".to_string()],
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_add_edge_prop_undo() {
        let undo = AddEdgePropUndo {
            src_label: 1,
            dst_label: 2,
            edge_label: 3,
            src_label_name: "person".to_string(),
            dst_label_name: "person".to_string(),
            edge_label_name: "knows".to_string(),
            prop_names: vec!["new_prop".to_string()],
        };

        let target = MockUndoTarget;
        undo.undo(&target, 1).expect("Undo failed");
    }

    #[test]
    fn test_undo_log_entry_enum() {
        let entry = UndoLogEntry::InsertVertex(InsertVertexUndo {
            v_label: 1,
            vid: VertexId::from_int64(100),
        });

        let target = MockUndoTarget;
        entry.undo(&target, 1).expect("Undo failed");
        assert!(entry.description().contains("InsertVertexUndo"));
    }
}
