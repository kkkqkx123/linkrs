//! Update Transaction
//!
//! Provides update transaction for MVCC-based graph database.
//! An update transaction can perform DDL operations (create/drop types),
//! update properties, and delete vertices/edges.
//! Update transactions require exclusive access and block all other transactions.

use std::collections::HashSet;

use postcard::to_allocvec;

use super::read_transaction::RELEASED_TIMESTAMP;
use super::rollback::RollbackHelper;
use super::undo_log::{
    AddEdgePropUndo, AddVertexPropUndo, CreateEdgeTypeUndo, CreateVertexTypeUndo,
    DeleteEdgePropUndo, DeleteVertexPropUndo, PropertyValue, RelatedEdgeInfo, UndoLogEntry,
    UndoLogError, UndoLogManager, UndoTarget,
};
use crate::core::wal::redo::{
    CreateEdgeTypeRedo, CreateVertexTypeRedo, DeleteEdgeRedo, DeleteVertexRedo, UpdateEdgePropRedo,
    UpdateVertexPropRedo,
};
use crate::core::wal::types::{WalHeader, WalOpType};
use super::wal::writer::WalWriter;
use super::wal::{ColumnId, LabelId, Timestamp, VertexId};
use super::mvcc::{VersionManager, VersionManagerError};

/// Result type for vertex deletion including related edge information
type DeleteVertexResult = Vec<(LabelId, LabelId, LabelId, Vec<RelatedEdgeInfo>)>;

/// Update transaction error
#[derive(Debug, Clone, thiserror::Error)]
pub enum UpdateTransactionError {
    #[error("Version manager error: {0}")]
    VersionManagerError(#[from] VersionManagerError),

    #[error("WAL error: {0}")]
    WalError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Undo log error: {0}")]
    UndoLogError(#[from] UndoLogError),

    #[error("Transaction already released")]
    AlreadyReleased,

    #[error("Label not found: {0}")]
    LabelNotFound(LabelId),

    #[error("Label already exists: {0}")]
    LabelAlreadyExists(String),

    #[error("Vertex not found: {0}")]
    VertexNotFound(VertexId),

    #[error("Edge not found")]
    EdgeNotFound,

    #[error("Property not found: {0}")]
    PropertyNotFound(String),

    #[error("Property type mismatch: expected {expected}, got {actual}")]
    PropertyTypeMismatch { expected: String, actual: String },

    #[error("Schema error: {0}")]
    SchemaError(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),
}

/// Update transaction result type
pub type UpdateTransactionResult<T> = Result<T, UpdateTransactionError>;

/// Schema definition for vertex/edge types
#[derive(Debug, Clone)]
pub struct PropertyDefinition {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

/// Create vertex type parameter
#[derive(Debug, Clone)]
pub struct CreateVertexTypeParam {
    pub space_name: String,
    pub label_name: String,
    pub properties: Vec<PropertyDefinition>,
    pub primary_keys: Vec<String>,
}

/// Create edge type parameter
#[derive(Debug, Clone)]
pub struct CreateEdgeTypeParam {
    pub space_name: String,
    pub src_label: String,
    pub dst_label: String,
    pub edge_label: String,
    pub properties: Vec<PropertyDefinition>,
}

/// Add vertex properties parameter
#[derive(Debug, Clone)]
pub struct AddVertexPropertiesParam {
    pub label_name: String,
    pub properties: Vec<PropertyDefinition>,
}

/// Add edge properties parameter
#[derive(Debug, Clone)]
pub struct AddEdgePropertiesParam {
    pub src_label: String,
    pub dst_label: String,
    pub edge_label: String,
    pub properties: Vec<PropertyDefinition>,
}

/// Delete vertex properties parameter
#[derive(Debug, Clone)]
pub struct DeleteVertexPropertiesParam {
    pub label_name: String,
    pub properties: Vec<String>,
}

/// Delete edge properties parameter
#[derive(Debug, Clone)]
pub struct DeleteEdgePropertiesParam {
    pub src_label: String,
    pub dst_label: String,
    pub edge_label: String,
    pub properties: Vec<String>,
}

/// Rename properties parameter
#[derive(Debug, Clone)]
pub struct RenamePropertiesParam {
    pub old_name: String,
    pub new_name: String,
}

/// Update Transaction
///
/// A transaction that can perform DDL and DML update operations.
/// Update transactions require exclusive access - only one update
/// transaction can run at a time, and it blocks all other transactions.
///
/// # Example
///
/// ```rust,ignore
/// let mut txn = UpdateTransaction::new(&mut graph, &version_manager, &mut wal_writer)?;
/// txn.create_vertex_type(param)?;
/// txn.commit()?;
/// ```
pub struct UpdateTransaction<'a, T: UpdateTarget + ?Sized> {
    graph: &'a mut T,
    version_manager: &'a VersionManager,
    wal_writer: &'a mut dyn WalWriter,
    timestamp: Timestamp,
    wal_buffer: Vec<u8>,
    undo_logs: UndoLogManager,
    op_num: usize,
    deleted_vertex_labels: HashSet<LabelId>,
    deleted_edge_labels: HashSet<(LabelId, LabelId, LabelId)>,
    schema_changed: bool,
}

/// Parameters for updating vertex property
pub struct UpdateVertexPropertyParams {
    pub label: LabelId,
    pub vid: VertexId,
}

/// Parameters for updating edge property
pub struct UpdateEdgePropertyParams {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
}

/// Parameters for deleting edge
pub struct DeleteEdgeParam {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
}

/// Parameters for updating edge property with old value for undo
pub struct UpdateEdgePropertyWithUndoParam<'a> {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub prop_name: &'a str,
    pub value: &'a [u8],
    pub old_value: PropertyValue,
}

/// Target for update operations (will be PropertyGraph in phase 2)
pub trait UpdateTarget: Send + Sync + UndoTarget {
    fn create_vertex_type(
        &mut self,
        param: &CreateVertexTypeParam,
    ) -> UpdateTransactionResult<LabelId>;

    fn create_edge_type(&mut self, param: &CreateEdgeTypeParam) -> UpdateTransactionResult<()>;

    fn delete_vertex_type(&mut self, label: LabelId) -> UpdateTransactionResult<()>;
    fn delete_edge_type(
        &mut self,
        src_label: LabelId,
        dst_label: LabelId,
        edge_label: LabelId,
    ) -> UpdateTransactionResult<()>;

    fn add_vertex_properties(
        &mut self,
        param: &AddVertexPropertiesParam,
    ) -> UpdateTransactionResult<()>;
    fn add_edge_properties(
        &mut self,
        param: &AddEdgePropertiesParam,
    ) -> UpdateTransactionResult<()>;
    fn delete_vertex_properties(
        &mut self,
        param: &DeleteVertexPropertiesParam,
    ) -> UpdateTransactionResult<()>;
    fn delete_edge_properties(
        &mut self,
        param: &DeleteEdgePropertiesParam,
    ) -> UpdateTransactionResult<()>;

    fn get_vertex_external_vid(
        &self,
        label: LabelId,
        internal_vid: VertexId,
        ts: Timestamp,
    ) -> Option<VertexId>;

    fn update_vertex_property(
        &mut self,
        param: UpdateVertexPropertyParams,
        prop_name: &str,
        value: &[u8],
        ts: Timestamp,
    ) -> UpdateTransactionResult<()>;

    fn update_edge_property(
        &mut self,
        param: UpdateEdgePropertyParams,
        prop_name: &str,
        value: &[u8],
        ts: Timestamp,
    ) -> UpdateTransactionResult<()>;

    fn delete_vertex(
        &mut self,
        label: LabelId,
        vid: VertexId,
        ts: Timestamp,
    ) -> UpdateTransactionResult<DeleteVertexResult>;
    fn delete_edge(&mut self, param: DeleteEdgeParam, ts: Timestamp)
        -> UpdateTransactionResult<()>;

    fn get_vertex_label_id(&self, name: &str) -> Option<LabelId>;
    fn get_edge_label_id(&self, name: &str) -> Option<LabelId>;
    fn get_vertex_label_name(&self, label: LabelId) -> Option<String>;
    fn get_edge_label_name(&self, label: LabelId) -> Option<String>;
    fn contains_vertex_label(&self, name: &str) -> bool;
    fn contains_edge_label(&self, src: &str, dst: &str, edge: &str) -> bool;
}

impl<'a, T: UpdateTarget + ?Sized> UpdateTransaction<'a, T> {
    /// Create a new update transaction
    ///
    /// Acquires an update timestamp from the version manager.
    /// This will block until all other transactions complete.
    pub fn new(
        graph: &'a mut T,
        version_manager: &'a VersionManager,
        wal_writer: &'a mut dyn WalWriter,
    ) -> UpdateTransactionResult<Self> {
        let timestamp = version_manager.acquire_update_timestamp()?;
        let wal_buffer = vec![0; WalHeader::SIZE];

        Ok(Self {
            graph,
            version_manager,
            wal_writer,
            timestamp,
            wal_buffer,
            undo_logs: UndoLogManager::new(),
            op_num: 0,
            deleted_vertex_labels: HashSet::new(),
            deleted_edge_labels: HashSet::new(),
            schema_changed: false,
        })
    }

    /// Get the transaction's timestamp
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Check if schema was changed
    pub fn schema_changed(&self) -> bool {
        self.schema_changed
    }

    /// Create a new vertex type
    pub fn create_vertex_type(
        &mut self,
        param: &CreateVertexTypeParam,
    ) -> UpdateTransactionResult<LabelId> {
        if self.graph.contains_vertex_label(&param.label_name) {
            return Err(UpdateTransactionError::LabelAlreadyExists(
                param.label_name.clone(),
            ));
        }

        self.serialize_redo(
            WalOpType::CreateVertexType,
            &CreateVertexTypeRedo {
                space_name: param.space_name.clone(),
                label_id: None,
                label_name: param.label_name.clone(),
                schema: param
                    .properties
                    .iter()
                    .map(|p| (p.name.clone(), p.data_type.clone()))
                    .collect(),
            },
        )?;
        self.op_num += 1;

        let _label_name = param.label_name.clone();
        let label_id = self.graph.create_vertex_type(param)?;

        self.undo_logs
            .add(UndoLogEntry::CreateVertexType(CreateVertexTypeUndo {
                vertex_type: label_id,
            }));

        self.deleted_vertex_labels.remove(&label_id);
        self.schema_changed = true;

        Ok(label_id)
    }

    /// Create a new edge type
    pub fn create_edge_type(&mut self, param: &CreateEdgeTypeParam) -> UpdateTransactionResult<()> {
        if self
            .graph
            .contains_edge_label(&param.src_label, &param.dst_label, &param.edge_label)
        {
            return Err(UpdateTransactionError::LabelAlreadyExists(
                param.edge_label.clone(),
            ));
        }

        self.serialize_redo(
            WalOpType::CreateEdgeType,
            &CreateEdgeTypeRedo {
                space_name: param.space_name.clone(),
                label_id: None,
                src_label: param.src_label.clone(),
                dst_label: param.dst_label.clone(),
                edge_label: param.edge_label.clone(),
                schema: param
                    .properties
                    .iter()
                    .map(|p| (p.name.clone(), p.data_type.clone()))
                    .collect(),
            },
        )?;
        self.op_num += 1;

        let src_label = param.src_label.clone();
        let dst_label = param.dst_label.clone();
        let edge_label = param.edge_label.clone();

        self.graph.create_edge_type(param)?;

        let src_label_id = self.graph.get_vertex_label_id(&src_label).unwrap_or(0);
        let dst_label_id = self.graph.get_vertex_label_id(&dst_label).unwrap_or(0);
        let edge_label_id = self.graph.get_edge_label_id(&edge_label).unwrap_or(0);

        self.undo_logs
            .add(UndoLogEntry::CreateEdgeType(CreateEdgeTypeUndo {
                src_type: src_label_id,
                dst_type: dst_label_id,
                edge_type: edge_label_id,
            }));

        self.deleted_edge_labels
            .remove(&(src_label_id, dst_label_id, edge_label_id));
        self.schema_changed = true;

        Ok(())
    }

    /// Add properties to a vertex type
    pub fn add_vertex_properties(
        &mut self,
        param: &AddVertexPropertiesParam,
    ) -> UpdateTransactionResult<()> {
        if !self.graph.contains_vertex_label(&param.label_name) {
            return Err(UpdateTransactionError::LabelNotFound(0));
        }

        let label_id = self
            .graph
            .get_vertex_label_id(&param.label_name)
            .unwrap_or(0);
        let prop_names: Vec<String> = param.properties.iter().map(|p| p.name.clone()).collect();

        self.graph.add_vertex_properties(param)?;

        self.undo_logs
            .add(UndoLogEntry::AddVertexProp(AddVertexPropUndo {
                label: label_id,
                label_name: param.label_name.clone(),
                prop_names,
            }));

        self.schema_changed = true;
        Ok(())
    }

    /// Add properties to an edge type
    pub fn add_edge_properties(
        &mut self,
        param: &AddEdgePropertiesParam,
    ) -> UpdateTransactionResult<()> {
        if !self
            .graph
            .contains_edge_label(&param.src_label, &param.dst_label, &param.edge_label)
        {
            return Err(UpdateTransactionError::LabelNotFound(0));
        }

        let src_label_id = self
            .graph
            .get_vertex_label_id(&param.src_label)
            .unwrap_or(0);
        let dst_label_id = self
            .graph
            .get_vertex_label_id(&param.dst_label)
            .unwrap_or(0);
        let edge_label_id = self.graph.get_edge_label_id(&param.edge_label).unwrap_or(0);
        let prop_names: Vec<String> = param.properties.iter().map(|p| p.name.clone()).collect();

        self.graph.add_edge_properties(param)?;

        self.undo_logs
            .add(UndoLogEntry::AddEdgeProp(AddEdgePropUndo {
                src_label: src_label_id,
                dst_label: dst_label_id,
                edge_label: edge_label_id,
                src_label_name: param.src_label.clone(),
                dst_label_name: param.dst_label.clone(),
                edge_label_name: param.edge_label.clone(),
                prop_names,
            }));

        self.schema_changed = true;
        Ok(())
    }

    /// Delete properties from a vertex type
    pub fn delete_vertex_properties(
        &mut self,
        param: &DeleteVertexPropertiesParam,
    ) -> UpdateTransactionResult<()> {
        if !self.graph.contains_vertex_label(&param.label_name) {
            return Err(UpdateTransactionError::LabelNotFound(0));
        }

        let label_id = self
            .graph
            .get_vertex_label_id(&param.label_name)
            .unwrap_or(0);

        self.graph.delete_vertex_properties(param)?;

        self.undo_logs
            .add(UndoLogEntry::DeleteVertexProp(DeleteVertexPropUndo {
                label: label_id,
                label_name: param.label_name.clone(),
                prop_names: param.properties.clone(),
            }));

        self.schema_changed = true;
        Ok(())
    }

    /// Delete properties from an edge type
    pub fn delete_edge_properties(
        &mut self,
        param: &DeleteEdgePropertiesParam,
    ) -> UpdateTransactionResult<()> {
        if !self
            .graph
            .contains_edge_label(&param.src_label, &param.dst_label, &param.edge_label)
        {
            return Err(UpdateTransactionError::LabelNotFound(0));
        }

        let src_label_id = self
            .graph
            .get_vertex_label_id(&param.src_label)
            .unwrap_or(0);
        let dst_label_id = self
            .graph
            .get_vertex_label_id(&param.dst_label)
            .unwrap_or(0);
        let edge_label_id = self.graph.get_edge_label_id(&param.edge_label).unwrap_or(0);

        self.graph.delete_edge_properties(param)?;

        self.undo_logs
            .add(UndoLogEntry::DeleteEdgeProp(DeleteEdgePropUndo {
                src_label: src_label_id,
                dst_label: dst_label_id,
                edge_label: edge_label_id,
                src_label_name: param.src_label.clone(),
                dst_label_name: param.dst_label.clone(),
                edge_label_name: param.edge_label.clone(),
                prop_names: param.properties.clone(),
            }));

        self.schema_changed = true;
        Ok(())
    }

    /// Update a vertex property
    pub fn update_vertex_property(
        &mut self,
        label: LabelId,
        vid: VertexId,
        prop_name: &str,
        value: &[u8],
        old_value: PropertyValue,
    ) -> UpdateTransactionResult<()> {
        let param = UpdateVertexPropertyParams { label, vid };
        self.graph
            .update_vertex_property(param, prop_name, value, self.timestamp)?;

        let external_vid = self
            .graph
            .get_vertex_external_vid(label, vid, self.timestamp)
            .unwrap_or(vid);

        let redo = UpdateVertexPropRedo {
            label,
            vid: external_vid,
            prop_name: prop_name.to_string(),
            value: value.to_vec(),
        };
        self.serialize_redo(WalOpType::UpdateVertexProp, &redo)?;
        self.op_num += 1;

        self.undo_logs
            .add(RollbackHelper::create_update_vertex_prop_undo(
                label,
                vid.as_u64().unwrap_or(0),
                ColumnId(0),
                old_value,
            ));

        Ok(())
    }

    /// Update an edge property
    pub fn update_edge_property(
        &mut self,
        param: UpdateEdgePropertyWithUndoParam,
    ) -> UpdateTransactionResult<()> {
        let edge_param = UpdateEdgePropertyParams {
            src_label: param.src_label,
            src_vid: param.src_vid,
            dst_label: param.dst_label,
            dst_vid: param.dst_vid,
            edge_label: param.edge_label,
            rank: param.rank,
        };
        self.graph.update_edge_property(
            edge_param,
            param.prop_name,
            param.value,
            self.timestamp,
        )?;

        let src_external = self
            .graph
            .get_vertex_external_vid(param.src_label, param.src_vid, self.timestamp)
            .unwrap_or(param.src_vid);
        let dst_external = self
            .graph
            .get_vertex_external_vid(param.dst_label, param.dst_vid, self.timestamp)
            .unwrap_or(param.dst_vid);

        let redo = UpdateEdgePropRedo {
            src_label: param.src_label,
            src_vid: src_external,
            dst_label: param.dst_label,
            dst_vid: dst_external,
            edge_label: param.edge_label,
            rank: param.rank,
            prop_name: param.prop_name.to_string(),
            value: param.value.to_vec(),
        };
        self.serialize_redo(WalOpType::UpdateEdgeProp, &redo)?;
        self.op_num += 1;

        self.undo_logs
            .add(RollbackHelper::create_update_edge_prop_undo(
                super::rollback::CreateUpdateEdgePropUndoParams {
                    src_label: param.src_label,
                    src_vid: param.src_vid.as_u64().unwrap_or(0),
                    dst_label: param.dst_label,
                    dst_vid: param.dst_vid.as_u64().unwrap_or(0),
                    edge_label: param.edge_label,
                    rank: param.rank,
                    oe_offset: 0,
                    ie_offset: 0,
                    col_id: ColumnId(0),
                    old_value: param.old_value,
                },
            ));

        Ok(())
    }

    /// Delete a vertex
    pub fn delete_vertex(&mut self, label: LabelId, vid: VertexId) -> UpdateTransactionResult<()> {
        let related_edges = UpdateTarget::delete_vertex(self.graph, label, vid, self.timestamp)?;

        let external_vid = self
            .graph
            .get_vertex_external_vid(label, vid, self.timestamp)
            .unwrap_or(vid);

        let redo = DeleteVertexRedo {
            label,
            vid: external_vid,
        };
        self.serialize_redo(WalOpType::DeleteVertex, &redo)?;
        self.op_num += 1;

        self.undo_logs
            .add(RollbackHelper::create_remove_vertex_undo(
                super::rollback::CreateRemoveVertexUndoParams {
                    label,
                    vid: vid.as_u64().unwrap_or(0),
                    related_edges: related_edges
                        .iter()
                        .map(
                            |(sl, dl, el, edges): &(
                                LabelId,
                                LabelId,
                                LabelId,
                                Vec<RelatedEdgeInfo>,
                            )| { (*sl, *dl, *el, edges.clone()) },
                        )
                        .collect(),
                },
            ));

        Ok(())
    }

    /// Delete an edge
    pub fn delete_edge(
        &mut self,
        src_label: LabelId,
        src_vid: VertexId,
        dst_label: LabelId,
        dst_vid: VertexId,
        edge_label: LabelId,
        rank: i64,
    ) -> UpdateTransactionResult<()> {
        let param = DeleteEdgeParam {
            src_label,
            src_vid,
            dst_label,
            dst_vid,
            edge_label,
            rank,
        };
        UpdateTarget::delete_edge(self.graph, param, self.timestamp)?;

        let src_external = self
            .graph
            .get_vertex_external_vid(src_label, src_vid, self.timestamp)
            .unwrap_or(src_vid);
        let dst_external = self
            .graph
            .get_vertex_external_vid(dst_label, dst_vid, self.timestamp)
            .unwrap_or(dst_vid);

        let redo = DeleteEdgeRedo {
            src_label,
            src_vid: src_external,
            dst_label,
            dst_vid: dst_external,
            edge_label,
            rank,
        };
        self.serialize_redo(WalOpType::DeleteEdge, &redo)?;
        self.op_num += 1;

        self.undo_logs.add(RollbackHelper::create_remove_edge_undo(
            super::rollback::CreateRemoveEdgeUndoParams {
                src_label,
                src_vid: src_vid.as_u64().unwrap_or(0),
                dst_label,
                dst_vid: dst_vid.as_u64().unwrap_or(0),
                edge_label,
                rank,
                oe_offset: 0,
                ie_offset: 0,
            },
        ));

        Ok(())
    }

    /// Commit the update transaction
    pub fn commit(mut self) -> UpdateTransactionResult<()> {
        if self.timestamp == RELEASED_TIMESTAMP {
            return Ok(());
        }

        if self.op_num == 0 {
            self.release();
            return Ok(());
        }

        self.write_wal_header(true);

        self.wal_writer
            .append(&self.wal_buffer)
            .map_err(|e| UpdateTransactionError::WalError(e.to_string()))?;

        self.apply_deletions();
        self.release();

        Ok(())
    }

    /// Abort the update transaction
    pub fn abort(mut self) -> UpdateTransactionResult<()> {
        self.revert_changes()?;
        self.release();
        Ok(())
    }

    /// Revert all changes made by this transaction
    fn revert_changes(&mut self) -> UpdateTransactionResult<()> {
        let ts = self.timestamp;
        while let Some(log) = self.undo_logs.pop() {
            if let Err(e) = log.undo(self.graph, ts) {
                return Err(UpdateTransactionError::WalError(format!(
                    "Failed to undo operation: {}",
                    e
                )));
            }
        }
        Ok(())
    }

    /// Apply pending deletions
    fn apply_deletions(&mut self) {
        // In a real implementation, this would apply the actual deletions
        // to the graph storage
    }

    /// Release the update timestamp
    fn release(&mut self) {
        if self.timestamp != RELEASED_TIMESTAMP {
            self.version_manager
                .release_update_timestamp(self.timestamp);
            self.timestamp = RELEASED_TIMESTAMP;
        }
    }

    /// Serialize a redo log entry
    fn serialize_redo<U: serde::Serialize + serde::de::DeserializeOwned>(
        &mut self,
        op_type: WalOpType,
        redo: &U,
    ) -> UpdateTransactionResult<()> {
        let op_byte = op_type as u8;
        self.wal_buffer.push(op_byte);

        let encoded = to_allocvec(redo)
            .map_err(|e| UpdateTransactionError::SerializationError(e.to_string()))?;

        let len = encoded.len() as u32;
        self.wal_buffer.extend_from_slice(&len.to_le_bytes());
        self.wal_buffer.extend_from_slice(&encoded);

        Ok(())
    }

    /// Write the WAL header
    fn write_wal_header(&mut self, _is_update: bool) {
        let header = WalHeader::new(WalOpType::CreateVertexType, self.timestamp, 0);
        let header_bytes = header.as_bytes();
        self.wal_buffer[..WalHeader::SIZE].copy_from_slice(header_bytes);
    }
}

impl<'a, T: UpdateTarget + ?Sized> Drop for UpdateTransaction<'a, T> {
    fn drop(&mut self) {
        if self.timestamp != RELEASED_TIMESTAMP {
            self.version_manager
                .release_update_timestamp(self.timestamp);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::undo_log::UndoLogResult;
    use super::super::wal::writer::DummyWalWriter;
    use super::*;
    use crate::core::types::{EdgeDeletionContext, EdgeIdentifier, EdgeKey, VertexIdentifier};
    use crate::transaction::ColumnId;

    struct MockUpdateTarget;

    impl UndoTarget for MockUpdateTarget {
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

    impl UpdateTarget for MockUpdateTarget {
        fn create_vertex_type(
            &mut self,
            _param: &CreateVertexTypeParam,
        ) -> UpdateTransactionResult<LabelId> {
            Ok(1)
        }

        fn create_edge_type(
            &mut self,
            _param: &CreateEdgeTypeParam,
        ) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn get_vertex_external_vid(
            &self,
            _label: LabelId,
            _internal_vid: VertexId,
            _ts: Timestamp,
        ) -> Option<VertexId> {
            None
        }

        fn delete_vertex_type(&mut self, _label: LabelId) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn delete_edge_type(
            &mut self,
            _src_label: LabelId,
            _dst_label: LabelId,
            _edge_label: LabelId,
        ) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn add_vertex_properties(
            &mut self,
            _param: &AddVertexPropertiesParam,
        ) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn add_edge_properties(
            &mut self,
            _param: &AddEdgePropertiesParam,
        ) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn delete_vertex_properties(
            &mut self,
            _param: &DeleteVertexPropertiesParam,
        ) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn delete_edge_properties(
            &mut self,
            _param: &DeleteEdgePropertiesParam,
        ) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn update_vertex_property(
            &mut self,
            _param: UpdateVertexPropertyParams,
            _prop_name: &str,
            _value: &[u8],
            _ts: Timestamp,
        ) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn update_edge_property(
            &mut self,
            _param: UpdateEdgePropertyParams,
            _prop_name: &str,
            _value: &[u8],
            _ts: Timestamp,
        ) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn delete_vertex(
            &mut self,
            _label: LabelId,
            _vid: VertexId,
            _ts: Timestamp,
        ) -> UpdateTransactionResult<Vec<(LabelId, LabelId, LabelId, Vec<RelatedEdgeInfo>)>>
        {
            Ok(vec![])
        }

        fn delete_edge(
            &mut self,
            _param: DeleteEdgeParam,
            _ts: Timestamp,
        ) -> UpdateTransactionResult<()> {
            Ok(())
        }

        fn get_vertex_label_id(&self, _name: &str) -> Option<LabelId> {
            Some(1)
        }

        fn get_edge_label_id(&self, _name: &str) -> Option<LabelId> {
            Some(1)
        }

        fn get_vertex_label_name(&self, _label: LabelId) -> Option<String> {
            Some("test".to_string())
        }

        fn get_edge_label_name(&self, _label: LabelId) -> Option<String> {
            Some("test".to_string())
        }

        fn contains_vertex_label(&self, _name: &str) -> bool {
            false
        }

        fn contains_edge_label(&self, _src: &str, _dst: &str, _edge: &str) -> bool {
            false
        }
    }

    #[test]
    fn test_update_transaction_basic() {
        let vm = VersionManager::new();
        let mut target = MockUpdateTarget;
        let mut wal = DummyWalWriter::new();

        let txn = UpdateTransaction::new(&mut target, &vm, &mut wal)
            .expect("Failed to create update transaction");

        assert!(txn.timestamp() >= 1);
    }

    #[test]
    fn test_update_transaction_commit() {
        let vm = VersionManager::new();
        let mut target = MockUpdateTarget;
        let mut wal = DummyWalWriter::new();

        let txn = UpdateTransaction::new(&mut target, &vm, &mut wal)
            .expect("Failed to create update transaction");

        txn.commit().expect("Commit failed");

        assert!(!vm.is_update_in_progress());
    }

    #[test]
    fn test_update_transaction_abort() {
        let vm = VersionManager::new();
        let mut target = MockUpdateTarget;
        let mut wal = DummyWalWriter::new();

        let txn = UpdateTransaction::new(&mut target, &vm, &mut wal)
            .expect("Failed to create update transaction");

        txn.abort().expect("Abort failed");

        assert!(!vm.is_update_in_progress());
    }
}
