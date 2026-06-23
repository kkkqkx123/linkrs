//! Batch Operation Module
//!
//! Supports efficient high-volume data import

pub use crate::api::core::BatchConfig;
use crate::api::core::{
    BatchError as CoreBatchError, BatchOperation as CoreBatchOperation,
    BatchResult as CoreBatchResult,
};
use crate::api::core::{CoreError, CoreResult};
use crate::api::embedded::session::Session;
use crate::core::{Edge, Vertex};
use crate::storage::StorageClient;

/// Batch Inserter
///
/// For efficient batch insertion of vertex and edge data
///
/// # Examples
///
/// ```rust
/// use graphdb::api::embedded::{GraphDatabase, DatabaseConfig};
/// use graphdb::core::{Vertex, Edge, Value};
///
/// # fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let db = GraphDatabase::open("my_db")?;
/// let session = db.session()?;
///
/// // Create a batch inserter that automatically flushes every 100 entries
/// let mut inserter = session.batch_inserter(100);
///
/// // Add vertices
/// for i in 0..1000 {
///     let vertex = Vertex::with_vid(Value::Int(i));
///     inserter.add_vertex(vertex);
/// }
///
/// // Perform batch insertion
/// let result = inserter.execute()?;
/// println!("Inserted {} vertices", result.vertices_inserted);
/// # Ok(())
/// # }
/// ```
pub struct BatchInserter<'sess, S: StorageClient + Clone + 'static> {
    session: &'sess Session<S>,
    core_operation: CoreBatchOperation,
}

/// Batch operation results
#[derive(Debug, Clone, Default)]
pub struct BatchResult {
    /// Number of vertices inserted
    pub vertices_inserted: usize,
    /// Number of inserted edges
    pub edges_inserted: usize,
    /// error message
    pub errors: Vec<BatchError>,
}

impl BatchResult {
    /// Get total number of inserted items
    pub fn total_inserted(&self) -> usize {
        self.vertices_inserted + self.edges_inserted
    }

    /// Check if there are any errors
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    /// Get error count
    pub fn error_count(&self) -> usize {
        self.errors.len()
    }

    /// Merge another batch result into this one
    pub fn merge(&mut self, other: BatchResult) {
        self.vertices_inserted += other.vertices_inserted;
        self.edges_inserted += other.edges_inserted;
        self.errors.extend(other.errors);
    }
}

impl From<CoreBatchResult> for BatchResult {
    fn from(result: CoreBatchResult) -> Self {
        Self {
            vertices_inserted: result.vertices_inserted,
            edges_inserted: result.edges_inserted,
            errors: result.errors.into_iter().map(Into::into).collect(),
        }
    }
}

/// batch error
#[derive(Debug, Clone)]
pub struct BatchError {
    /// Index where the error occurred
    pub index: usize,
    /// Error item type
    pub item_type: BatchItemType,
    /// error message
    pub error: String,
}

impl BatchError {
    /// Create a new batch error
    pub fn new(index: usize, item_type: BatchItemType, error: impl Into<String>) -> Self {
        Self {
            index,
            item_type,
            error: error.into(),
        }
    }
}

impl From<CoreBatchError> for BatchError {
    fn from(error: CoreBatchError) -> Self {
        Self {
            index: error.index,
            item_type: error.item_type.into(),
            error: error.message,
        }
    }
}

/// Batch item type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchItemType {
    /// vertice
    Vertex,
    /// suffix of a noun of locality
    Edge,
}

impl From<crate::api::core::BatchItemType> for BatchItemType {
    fn from(item_type: crate::api::core::BatchItemType) -> Self {
        match item_type {
            crate::api::core::BatchItemType::Vertex => BatchItemType::Vertex,
            crate::api::core::BatchItemType::Edge => BatchItemType::Edge,
        }
    }
}

impl<'sess, S: StorageClient + Clone + 'static + graphdb_storage::storage::UndoTarget>
    BatchInserter<'sess, S>
{
    /// Creating a new batch inserter
    pub(crate) fn new(session: &'sess Session<S>, batch_size: usize) -> Self {
        let config = BatchConfig::new().with_batch_size(batch_size);
        Self {
            session,
            core_operation: CoreBatchOperation::new(config),
        }
    }

    /// Adding Vertices
    ///
    /// # Parameters
    /// - `vertex` - the vertex to be inserted
    ///
    /// # Back
    /// - Return to itself, supporting chain calls
    pub fn add_vertex(&mut self, vertex: Vertex) -> &mut Self {
        self.core_operation.add_vertex(vertex);

        // Automatically flushes if batch size is reached
        if self.core_operation.should_flush() {
            let _ = self.flush();
        }

        self
    }

    /// Add Edge
    ///
    /// # Parameters
    /// - `edge` - the edge to be inserted
    ///
    /// # Back
    /// Return itself, supporting chained calls.
    pub fn add_edge(&mut self, edge: Edge) -> &mut Self {
        self.core_operation.add_edge(edge);

        // Automatically flushes if batch size is reached
        if self.core_operation.should_flush() {
            let _ = self.flush();
        }

        self
    }

    /// Adding multiple vertices
    ///
    /// # Parameters
    /// - `vertices` - a list of vertices to be inserted
    pub fn add_vertices(&mut self, vertices: Vec<Vertex>) -> &mut Self {
        for vertex in vertices {
            self.add_vertex(vertex);
        }
        self
    }

    /// Adding multiple edges
    ///
    /// # Parameters
    /// - `edges` - a list of edges to be inserted
    pub fn add_edges(&mut self, edges: Vec<Edge>) -> &mut Self {
        for edge in edges {
            self.add_edge(edge);
        }
        self
    }

    /// Perform batch insertion
    ///
    /// Flush all buffered data and return results
    ///
    /// # Return
    /// - Returns batch operation results on success
    /// - Return error on failure
    pub fn execute(mut self) -> CoreResult<BatchResult> {
        // Get current space name
        let space_name = self
            .session
            .space_name()
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        // Execute batch operation using core API
        let mut storage = self.session.storage_mut();
        let core_result = self
            .core_operation
            .execute_sync(&mut *storage, &space_name)?;

        Ok(core_result.into())
    }

    /// Flush the current buffer
    fn flush(&mut self) -> CoreResult<()> {
        if self.core_operation.is_empty() {
            return Ok(());
        }

        // Get current space name
        let space_name = self
            .session
            .space_name()
            .ok_or_else(|| CoreError::InvalidParameter("No graph space selected".to_string()))?;

        // Execute batch operation using core API
        let mut storage = self.session.storage_mut();
        let _ = self
            .core_operation
            .execute_sync(&mut *storage, &space_name)?;

        Ok(())
    }

    /// Get the number of items in the current buffer.
    pub fn buffered_count(&self) -> usize {
        self.core_operation.len()
    }

    /// Get the number of vertices in the current buffer.
    pub fn buffered_vertices(&self) -> usize {
        // Note: This is an approximation since core API doesn't expose this detail
        self.core_operation.len()
    }

    /// Get the number of edges in the current buffer.
    pub fn buffered_edges(&self) -> usize {
        // Note: This is an approximation since core API doesn't expose this detail
        self.core_operation.len()
    }

    /// Get the batch size
    pub fn batch_size(&self) -> usize {
        self.core_operation.batch_size()
    }

    /// Check if there is buffered data
    pub fn has_buffered_data(&self) -> bool {
        !self.core_operation.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_result_from_core() {
        let core_result = CoreBatchResult {
            vertices_inserted: 10,
            edges_inserted: 5,
            failed_count: 1,
            errors: vec![CoreBatchError {
                index: 0,
                item_type: crate::api::core::BatchItemType::Vertex,
                message: "Test error".to_string(),
            }],
        };

        let result: BatchResult = core_result.into();
        assert_eq!(result.vertices_inserted, 10);
        assert_eq!(result.edges_inserted, 5);
        assert_eq!(result.errors.len(), 1);
    }
}
