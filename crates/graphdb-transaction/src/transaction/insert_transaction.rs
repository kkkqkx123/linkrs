//! Insert Transaction
//!
//! Provides insert-only transaction for MVCC-based graph database.
//! An insert transaction can only add new vertices and edges, not modify
//! or delete existing data. This allows for higher concurrency compared
//! to update transactions.

use std::collections::HashMap;

use postcard::{from_bytes, to_allocvec};

use super::read_transaction::RELEASED_TIMESTAMP;
use crate::core::wal::redo::{InsertEdgeRedo, InsertVertexRedo};
use crate::core::wal::types::{WalHeader, WalOpType};
use super::wal::writer::WalWriter;
use super::wal::{EdgeId, LabelId, Timestamp};
use super::mvcc::{VersionManager, VersionManagerError};
use crate::core::types::VertexId;

/// Insert transaction error
#[derive(Debug, Clone, thiserror::Error)]
pub enum InsertTransactionError {
    #[error("Version manager error: {0}")]
    VersionManagerError(#[from] VersionManagerError),

    #[error("WAL error: {0}")]
    WalError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Transaction already released")]
    AlreadyReleased,

    #[error("Label not found: {0}")]
    LabelNotFound(LabelId),

    #[error("Vertex already exists: {0}")]
    VertexAlreadyExists(VertexId),

    #[error("Vertex not found: {0}")]
    VertexNotFound(VertexId),

    #[error("Property type mismatch: expected {expected}, got {actual}")]
    PropertyTypeMismatch { expected: String, actual: String },

    #[error("Property count mismatch: expected {expected}, got {actual}")]
    PropertyCountMismatch { expected: usize, actual: usize },

    #[error("Schema error: {0}")]
    SchemaError(String),
}

/// Insert transaction result type
pub type InsertTransactionResult<T> = Result<T, InsertTransactionError>;

/// Insert Transaction
///
/// A transaction that can only insert new data (vertices and edges).
/// Insert transactions can run concurrently with each other and with
/// read transactions, but not with update transactions.
///
/// # Example
///
/// ```rust,ignore
/// let mut txn = InsertTransaction::new(&mut graph, &version_manager, &mut wal_writer)?;
/// txn.add_vertex(label, id, properties)?;
/// txn.commit()?;
/// ```
pub struct InsertTransaction<'a, T: InsertTarget + ?Sized> {
    graph: &'a mut T,
    version_manager: &'a VersionManager,
    wal_writer: &'a mut dyn WalWriter,
    timestamp: Timestamp,
    wal_buffer: Vec<u8>,
    added_vertices: HashMap<LabelId, VertexId>,
    vertex_nums: HashMap<LabelId, u64>,
}

/// Parameters for adding an edge in insert transaction
pub struct AddEdgeInsertParam<'a> {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub properties: &'a [(String, Vec<u8>)],
    pub ts: Timestamp,
}

/// Target for insert operations (will be PropertyGraph in phase 2)
pub trait InsertTarget: Send + Sync {
    fn add_vertex(
        &mut self,
        label: LabelId,
        vid: VertexId,
        properties: &[(String, Vec<u8>)],
        ts: Timestamp,
    ) -> InsertTransactionResult<VertexId>;

    fn add_edge(&mut self, param: AddEdgeInsertParam) -> InsertTransactionResult<EdgeId>;

    fn get_vertex_id(&self, label: LabelId, vid: VertexId, ts: Timestamp) -> Option<VertexId>;

    fn get_vertex_external_vid(
        &self,
        label: LabelId,
        internal_vid: VertexId,
        ts: Timestamp,
    ) -> Option<VertexId>;

    fn get_vertex_property_types(&self, label: LabelId) -> Vec<String>;
    fn get_edge_property_types(
        &self,
        src_label: LabelId,
        dst_label: LabelId,
        edge_label: LabelId,
    ) -> Vec<String>;
    fn vertex_label_num(&self) -> usize;
    fn lid_num(&self, label: LabelId) -> usize;
}

impl<'a, T: InsertTarget + ?Sized> InsertTransaction<'a, T> {
    /// Create a new insert transaction
    ///
    /// Acquires an insert timestamp from the version manager.
    pub fn new(
        graph: &'a mut T,
        version_manager: &'a VersionManager,
        wal_writer: &'a mut dyn WalWriter,
    ) -> InsertTransactionResult<Self> {
        let timestamp = version_manager.acquire_insert_timestamp();
        let wal_buffer = vec![0; WalHeader::SIZE];

        Ok(Self {
            graph,
            version_manager,
            wal_writer,
            timestamp,
            wal_buffer,
            added_vertices: HashMap::new(),
            vertex_nums: HashMap::new(),
        })
    }

    /// Get the transaction's timestamp
    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }

    /// Get vertex index by label and external VertexId
    pub fn get_vertex_index(&self, label: LabelId, vid: VertexId) -> Option<VertexId> {
        if let Some(internal_id) = self.graph.get_vertex_id(label, vid, self.timestamp) {
            return Some(internal_id);
        }

        if let Some(&base) = self.added_vertices.get(&label) {
            let added = self.vertex_nums.get(&label).copied().unwrap_or(0);
            if added > 0 {
                return Some(base + added);
            }
        }

        None
    }

    /// Get external VertexId by label and internal ID
    pub fn get_vertex_external_vid(&self, label: LabelId, vid: VertexId) -> Option<VertexId> {
        if let Some(&base) = self.added_vertices.get(&label) {
            if vid >= base {
                return None;
            }
        }
        self.graph
            .get_vertex_external_vid(label, vid, self.timestamp)
    }

    /// Add a new vertex
    ///
    /// # Arguments
    /// * `label` - Vertex label ID
    /// * `vid` - External vertex ID (int64 or string)
    /// * `properties` - Vertex properties as (name, value) pairs
    ///
    /// # Returns
    /// The internal vertex ID if successful
    pub fn add_vertex(
        &mut self,
        label: LabelId,
        vid: VertexId,
        properties: &[(String, Vec<u8>)],
    ) -> InsertTransactionResult<VertexId> {
        let expected_types = self.graph.get_vertex_property_types(label);
        if expected_types.len() != properties.len() {
            return Err(InsertTransactionError::PropertyCountMismatch {
                expected: expected_types.len(),
                actual: properties.len(),
            });
        }

        if self.get_vertex_index(label, vid).is_some() {
            return Err(InsertTransactionError::VertexAlreadyExists(VertexId::zero()));
        }

        let base = self
            .added_vertices
            .entry(label)
            .or_insert_with(|| VertexId::from_int64(self.graph.lid_num(label) as i64));
        let num = self.vertex_nums.entry(label).or_insert(0u64);
        let internal_vid = *base + *num;
        *num += 1;

        let redo = InsertVertexRedo {
            label,
            vid,
            properties: properties.to_vec(),
        };
        self.serialize_redo(WalOpType::InsertVertex, &redo)?;

        Ok(internal_vid)
    }

    /// Add a new edge
    ///
    /// # Arguments
    /// * `param` - Edge insertion parameters
    pub fn add_edge(&mut self, param: AddEdgeInsertParam) -> InsertTransactionResult<()> {
        let expected_types =
            self.graph
                .get_edge_property_types(param.src_label, param.dst_label, param.edge_label);
        if expected_types.len() != param.properties.len() {
            return Err(InsertTransactionError::PropertyCountMismatch {
                expected: expected_types.len(),
                actual: param.properties.len(),
            });
        }

        let src_vid = self
            .graph
            .get_vertex_external_vid(param.src_label, param.src_vid, self.timestamp)
            .ok_or(InsertTransactionError::VertexNotFound(param.src_vid))?;
        let dst_vid = self
            .graph
            .get_vertex_external_vid(param.dst_label, param.dst_vid, self.timestamp)
            .ok_or(InsertTransactionError::VertexNotFound(param.dst_vid))?;

        let redo = InsertEdgeRedo {
            src_label: param.src_label,
            src_vid,
            dst_label: param.dst_label,
            dst_vid,
            edge_label: param.edge_label,
            rank: param.rank,
            properties: param.properties.to_vec(),
        };
        self.serialize_redo(WalOpType::InsertEdge, &redo)?;

        Ok(())
    }

    /// Commit the insert transaction
    ///
    /// Writes the WAL and releases the timestamp.
    pub fn commit(mut self) -> InsertTransactionResult<()> {
        if self.timestamp == RELEASED_TIMESTAMP {
            return Ok(());
        }

        if self.wal_buffer.len() == WalHeader::SIZE {
            self.version_manager
                .release_insert_timestamp(self.timestamp);
            self.clear();
            return Ok(());
        }

        self.write_wal_header();

        self.wal_writer
            .append(&self.wal_buffer)
            .map_err(|e| InsertTransactionError::WalError(e.to_string()))?;

        self.ingest_wal()?;

        self.version_manager
            .release_insert_timestamp(self.timestamp);
        self.clear();

        Ok(())
    }

    /// Abort the insert transaction
    ///
    /// Simply releases the timestamp without writing WAL.
    pub fn abort(mut self) -> InsertTransactionResult<()> {
        if self.timestamp != RELEASED_TIMESTAMP {
            self.version_manager
                .release_insert_timestamp(self.timestamp);
            self.clear();
        }
        Ok(())
    }

    /// Serialize a redo log entry
    fn serialize_redo<U: serde::Serialize + serde::de::DeserializeOwned>(
        &mut self,
        op_type: WalOpType,
        redo: &U,
    ) -> InsertTransactionResult<()> {
        let op_byte = op_type as u8;
        self.wal_buffer.push(op_byte);

        let encoded = to_allocvec(redo)
            .map_err(|e| InsertTransactionError::SerializationError(e.to_string()))?;

        let len = encoded.len() as u32;
        self.wal_buffer.extend_from_slice(&len.to_le_bytes());
        self.wal_buffer.extend_from_slice(&encoded);

        Ok(())
    }

    /// Write the WAL header
    fn write_wal_header(&mut self) {
        let header = WalHeader::new(WalOpType::InsertVertex, self.timestamp, 0);
        let header_bytes = header.as_bytes();
        self.wal_buffer[..WalHeader::SIZE].copy_from_slice(header_bytes);
    }

    /// Ingest WAL entries into the graph
    fn ingest_wal(&mut self) -> InsertTransactionResult<()> {
        let data = &self.wal_buffer[WalHeader::SIZE..];
        let mut offset = 0;

        while offset < data.len() {
            let op_type = WalOpType::try_from(data[offset])
                .map_err(|e| InsertTransactionError::WalError(e.to_string()))?;
            offset += 1;

            let len = u32::from_le_bytes([
                data[offset],
                data[offset + 1],
                data[offset + 2],
                data[offset + 3],
            ]) as usize;
            offset += 4;

            let payload = &data[offset..offset + len];
            offset += len;

            match op_type {
                WalOpType::InsertVertex => {
                    let redo: InsertVertexRedo = from_bytes(payload)
                        .map_err(|e| InsertTransactionError::SerializationError(e.to_string()))?;
                    self.graph.add_vertex(
                        redo.label,
                        redo.vid,
                        &redo.properties,
                        self.timestamp,
                    )?;
                }
                WalOpType::InsertEdge => {
                    let redo: InsertEdgeRedo = from_bytes(payload)
                        .map_err(|e| InsertTransactionError::SerializationError(e.to_string()))?;
                    let src_internal = self
                        .graph
                        .get_vertex_id(redo.src_label, redo.src_vid, self.timestamp)
                        .ok_or(InsertTransactionError::VertexNotFound(VertexId::zero()))?;
                    let dst_internal = self
                        .graph
                        .get_vertex_id(redo.dst_label, redo.dst_vid, self.timestamp)
                        .ok_or(InsertTransactionError::VertexNotFound(VertexId::zero()))?;
                    let edge_param = AddEdgeInsertParam {
                        src_label: redo.src_label,
                        src_vid: src_internal,
                        dst_label: redo.dst_label,
                        dst_vid: dst_internal,
                        edge_label: redo.edge_label,
                        rank: redo.rank,
                        properties: &redo.properties,
                        ts: self.timestamp,
                    };
                    self.graph.add_edge(edge_param)?;
                }
                _ => {
                    return Err(InsertTransactionError::WalError(format!(
                        "Unexpected op type: {:?}",
                        op_type
                    )));
                }
            }
        }

        Ok(())
    }

    /// Clear internal state
    fn clear(&mut self) {
        self.wal_buffer.clear();
        self.wal_buffer.resize(WalHeader::SIZE, 0);
        self.added_vertices.clear();
        self.vertex_nums.clear();
        self.timestamp = RELEASED_TIMESTAMP;
    }
}

impl<'a, T: InsertTarget + ?Sized> Drop for InsertTransaction<'a, T> {
    fn drop(&mut self) {
        if self.timestamp != RELEASED_TIMESTAMP {
            self.version_manager
                .release_insert_timestamp(self.timestamp);
        }
    }
}

impl From<InsertTransactionError> for crate::core::error::StorageError {
    fn from(e: InsertTransactionError) -> Self {
        Self::db_error(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::super::wal::writer::DummyWalWriter;
    use super::*;

    struct MockInsertTarget;

    impl InsertTarget for MockInsertTarget {
        fn add_vertex(
            &mut self,
            _label: LabelId,
            _vid: VertexId,
            _properties: &[(String, Vec<u8>)],
            _ts: Timestamp,
        ) -> InsertTransactionResult<VertexId> {
            Ok(VertexId::from_int64(1))
        }

        fn add_edge(&mut self, _param: AddEdgeInsertParam) -> InsertTransactionResult<EdgeId> {
            Ok(EdgeId(1))
        }

        fn get_vertex_id(
            &self,
            _label: LabelId,
            _vid: VertexId,
            _ts: Timestamp,
        ) -> Option<VertexId> {
            None
        }

        fn get_vertex_external_vid(
            &self,
            _label: LabelId,
            _vid: VertexId,
            _ts: Timestamp,
        ) -> Option<VertexId> {
            Some(VertexId::from_int64(1))
        }

        fn get_vertex_property_types(&self, _label: LabelId) -> Vec<String> {
            vec![]
        }

        fn get_edge_property_types(
            &self,
            _src_label: LabelId,
            _dst_label: LabelId,
            _edge_label: LabelId,
        ) -> Vec<String> {
            vec![]
        }

        fn vertex_label_num(&self) -> usize {
            0
        }

        fn lid_num(&self, _label: LabelId) -> usize {
            0
        }
    }

    #[test]
    fn test_insert_transaction_basic() {
        let vm = VersionManager::new();
        let mut target = MockInsertTarget;
        let mut wal = DummyWalWriter::new();

        let txn = InsertTransaction::new(&mut target, &vm, &mut wal)
            .expect("Failed to create insert transaction");

        assert!(txn.timestamp() >= 1);
    }

    #[test]
    fn test_insert_transaction_commit() {
        let vm = VersionManager::new();
        let mut target = MockInsertTarget;
        let mut wal = DummyWalWriter::new();

        let txn = InsertTransaction::new(&mut target, &vm, &mut wal)
            .expect("Failed to create insert transaction");

        txn.commit().expect("Commit failed");

        assert_eq!(vm.pending_count(), 0);
    }

    #[test]
    fn test_insert_transaction_abort() {
        let vm = VersionManager::new();
        let mut target = MockInsertTarget;
        let mut wal = DummyWalWriter::new();

        let txn = InsertTransaction::new(&mut target, &vm, &mut wal)
            .expect("Failed to create insert transaction");

        txn.abort().expect("Abort failed");

        assert_eq!(vm.pending_count(), 0);
    }
}
