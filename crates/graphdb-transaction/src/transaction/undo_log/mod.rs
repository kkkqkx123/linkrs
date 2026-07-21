//! Undo Log
//!
//! Provides transaction rollback support through undo log entries.
//! Each undo log entry can reverse a specific operation during transaction abort.

pub mod file_backed;

pub use file_backed::{FileBackedUndoLog, UndoLogConfig};

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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

/// Undo log for restoring an edge removed by a transaction.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RestoreEdgeUndo {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub properties: Vec<(String, crate::core::Value)>,
}

impl RestoreEdgeUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, ts: Timestamp) -> UndoLogResult<()> {
        graph.restore_edge(
            EdgeIdentifier::new(
                self.src_label,
                self.src_vid,
                self.dst_label,
                self.dst_vid,
                self.edge_label,
                self.rank,
            ),
            self.properties.clone(),
            ts,
        )
    }

    pub fn description(&self) -> String {
        format!(
            "RestoreEdgeUndo(src={}, dst={}, edge={})",
            self.src_vid, self.dst_vid, self.edge_label
        )
    }
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RelatedEdgeInfo {
    pub src_vid: VertexId,
    pub dst_vid: VertexId,
    pub rank: i64,
    pub oe_offset: i32,
    pub ie_offset: i32,
}

/// Undo log for remove vertex operation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

/// Undo log for sequence increment operation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SequenceIncrementUndo {
    pub sequence_name: String,
    pub previous_value: i64,
}

impl SequenceIncrementUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_sequence_increment(&self.sequence_name, self.previous_value)
    }

    pub fn description(&self) -> String {
        format!(
            "SequenceIncrementUndo(name={}, prev={})",
            self.sequence_name, self.previous_value
        )
    }
}

/// Undo log for sequence create operation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SequenceCreateUndo {
    pub sequence_name: String,
}

impl SequenceCreateUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_sequence_create(&self.sequence_name)
    }

    pub fn description(&self) -> String {
        format!("SequenceCreateUndo(name={})", self.sequence_name)
    }
}

/// Undo log for sequence drop operation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SequenceDropUndo {
    pub sequence_name: String,
}

impl SequenceDropUndo {
    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, _ts: Timestamp) -> UndoLogResult<()> {
        graph.revert_sequence_drop(&self.sequence_name)
    }

    pub fn description(&self) -> String {
        format!("SequenceDropUndo(name={})", self.sequence_name)
    }
}

/// Undo log entry enum - zero-cost abstraction for all undo types
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum UndoLogEntry {
    CreateVertexType(CreateVertexTypeUndo),
    CreateEdgeType(CreateEdgeTypeUndo),
    InsertVertex(InsertVertexUndo),
    InsertEdge(InsertEdgeUndo),
    RestoreEdge(RestoreEdgeUndo),
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
    SequenceIncrement(SequenceIncrementUndo),
    SequenceCreate(SequenceCreateUndo),
    SequenceDrop(SequenceDropUndo),
}

impl UndoLogEntry {
    /// Returns true if this entry is a DML operation (vertex/edge mutation).
    /// DDL operations (schema/index changes) return false.
    pub fn is_dml(&self) -> bool {
        matches!(
            self,
            UndoLogEntry::InsertVertex(_)
                | UndoLogEntry::InsertEdge(_)
                | UndoLogEntry::RestoreEdge(_)
                | UndoLogEntry::UpdateVertexProp(_)
                | UndoLogEntry::UpdateEdgeProp(_)
                | UndoLogEntry::RemoveVertex(_)
                | UndoLogEntry::RemoveEdge(_)
                | UndoLogEntry::AddVertexProp(_)
                | UndoLogEntry::AddEdgeProp(_)
                | UndoLogEntry::RenameVertexProp(_)
                | UndoLogEntry::RenameEdgeProp(_)
                | UndoLogEntry::DeleteVertexProp(_)
                | UndoLogEntry::DeleteEdgeProp(_)
        )
    }

    pub fn undo<T: UndoTarget + ?Sized>(&self, graph: &T, ts: Timestamp) -> UndoLogResult<()> {
        match self {
            UndoLogEntry::CreateVertexType(u) => u.undo(graph, ts),
            UndoLogEntry::CreateEdgeType(u) => u.undo(graph, ts),
            UndoLogEntry::InsertVertex(u) => u.undo(graph, ts),
            UndoLogEntry::InsertEdge(u) => u.undo(graph, ts),
            UndoLogEntry::RestoreEdge(u) => u.undo(graph, ts),
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
            UndoLogEntry::SequenceIncrement(u) => u.undo(graph, ts),
            UndoLogEntry::SequenceCreate(u) => u.undo(graph, ts),
            UndoLogEntry::SequenceDrop(u) => u.undo(graph, ts),
        }
    }

    pub fn description(&self) -> String {
        match self {
            UndoLogEntry::CreateVertexType(u) => u.description(),
            UndoLogEntry::CreateEdgeType(u) => u.description(),
            UndoLogEntry::InsertVertex(u) => u.description(),
            UndoLogEntry::InsertEdge(u) => u.description(),
            UndoLogEntry::RestoreEdge(u) => u.description(),
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
            UndoLogEntry::SequenceIncrement(u) => u.description(),
            UndoLogEntry::SequenceCreate(u) => u.description(),
            UndoLogEntry::SequenceDrop(u) => u.description(),
        }
    }
}

/// Undo log manager for collecting and executing undo logs.
///
/// Uses `FileBackedUndoLog` internally: when entries exceed the memory
/// threshold, older entries are spilled to a temp file. The temp file is
/// automatically cleaned up when the manager is dropped (on commit/abort).
pub struct UndoLogManager {
    storage: FileBackedUndoLog,
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
        Self {
            storage: FileBackedUndoLog::new(UndoLogConfig::default()),
        }
    }

    pub fn with_config(config: UndoLogConfig) -> Self {
        Self {
            storage: FileBackedUndoLog::new(config),
        }
    }

    pub fn add(&mut self, log: UndoLogEntry) -> UndoLogResult<()> {
        self.storage.add(log)
    }

    pub fn add_insert_vertex(&mut self, label: LabelId, vid: VertexId) -> UndoLogResult<()> {
        self.add(UndoLogEntry::InsertVertex(InsertVertexUndo {
            v_label: label,
            vid,
        }))
    }

    pub fn add_insert_edge(&mut self, params: AddInsertEdgeParams) -> UndoLogResult<()> {
        self.add(UndoLogEntry::InsertEdge(InsertEdgeUndo {
            src_label: params.src_label,
            dst_label: params.dst_label,
            edge_label: params.edge_label,
            rank: params.rank,
            src_vid: params.src_vid,
            dst_vid: params.dst_vid,
            oe_offset: params.oe_offset,
            ie_offset: params.ie_offset,
        }))
    }

    pub fn add_update_vertex_prop(
        &mut self,
        label: LabelId,
        vid: VertexId,
        col_id: ColumnId,
        old_value: PropertyValue,
    ) -> UndoLogResult<()> {
        self.add(UndoLogEntry::UpdateVertexProp(UpdateVertexPropUndo {
            v_label: label,
            vid,
            col_id,
            old_value,
        }))
    }

    pub fn add_update_edge_prop(&mut self, params: AddUpdateEdgePropParams) -> UndoLogResult<()> {
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
        }))
    }

    pub fn add_sequence_increment(
        &mut self,
        sequence_name: String,
        previous_value: i64,
    ) -> UndoLogResult<()> {
        self.add(UndoLogEntry::SequenceIncrement(SequenceIncrementUndo {
            sequence_name,
            previous_value,
        }))
    }

    pub fn add_sequence_create(&mut self, sequence_name: String) -> UndoLogResult<()> {
        self.add(UndoLogEntry::SequenceCreate(SequenceCreateUndo {
            sequence_name,
        }))
    }

    pub fn add_sequence_drop(&mut self, sequence_name: String) -> UndoLogResult<()> {
        self.add(UndoLogEntry::SequenceDrop(SequenceDropUndo {
            sequence_name,
        }))
    }

    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    pub fn len(&self) -> usize {
        self.storage.len()
    }

    /// Returns true if the undo log contains any DML entries (vertex/edge mutations).
    /// Used to enforce DDL/DML boundary: DDL cannot execute if DML has already occurred.
    pub fn has_dml_entries(&self) -> bool {
        self.storage.buffer_has_dml()
    }

    pub fn clear(&mut self) -> UndoLogResult<()> {
        self.storage.clear()
    }

    pub fn pop(&mut self) -> UndoLogResult<Option<UndoLogEntry>> {
        self.storage.pop()
    }

    pub fn execute_undo<T: UndoTarget + ?Sized>(
        &mut self,
        graph: &T,
        ts: Timestamp,
    ) -> UndoLogResult<()> {
        self.storage.execute_undo(graph, ts)
    }

    pub fn execute_undo_from_index<T: UndoTarget + ?Sized>(
        &mut self,
        graph: &T,
        ts: Timestamp,
        start_index: usize,
    ) -> UndoLogResult<()> {
        self.storage.execute_undo_from_index(graph, ts, start_index)
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

        fn revert_sequence_increment(
            &self,
            _sequence_name: &str,
            _previous_value: i64,
        ) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_sequence_create(&self, _sequence_name: &str) -> UndoLogResult<()> {
            Ok(())
        }

        fn revert_sequence_drop(&self, _sequence_name: &str) -> UndoLogResult<()> {
            Ok(())
        }
    }

    #[test]
    fn test_undo_log_manager() {
        let mut manager = UndoLogManager::new();

        manager
            .add_insert_vertex(1, VertexId::from_int64(100))
            .expect("Failed to append undo log");
        manager
            .add_insert_edge(AddInsertEdgeParams {
                src_label: 1,
                dst_label: 2,
                edge_label: 3,
                rank: 0,
                src_vid: VertexId::from_int64(100),
                dst_vid: VertexId::from_int64(200),
                oe_offset: 0,
                ie_offset: 0,
            })
            .expect("Failed to append undo log");

        assert_eq!(manager.len(), 2);

        let target = MockUndoTarget;
        manager.execute_undo(&target, 1).expect("Undo failed");

        assert!(manager.is_empty());
    }

    #[test]
    fn test_execute_undo_from_index_keeps_prefix() {
        let mut manager = UndoLogManager::new();
        manager
            .add_insert_vertex(1, VertexId::from_int64(1))
            .expect("Failed to append undo log");
        manager
            .add_insert_vertex(1, VertexId::from_int64(2))
            .expect("Failed to append undo log");
        manager
            .add_insert_vertex(1, VertexId::from_int64(3))
            .expect("Failed to append undo log");

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

        manager
            .add_insert_vertex(1, VertexId::from_int64(100))
            .expect("Failed to append undo log");
        manager
            .add_insert_vertex(1, VertexId::from_int64(200))
            .expect("Failed to append undo log");
        manager
            .add_insert_vertex(1, VertexId::from_int64(300))
            .expect("Failed to append undo log");

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

        manager
            .add_insert_vertex(1, VertexId::from_int64(100))
            .expect("Failed to append undo log");
        manager
            .add_insert_edge(AddInsertEdgeParams {
                src_label: 1,
                dst_label: 2,
                edge_label: 3,
                rank: 0,
                src_vid: VertexId::from_int64(100),
                dst_vid: VertexId::from_int64(200),
                oe_offset: 0,
                ie_offset: 0,
            })
            .expect("Failed to append undo log");

        assert_eq!(manager.len(), 2);

        manager.clear().expect("Failed to clear undo log");

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
