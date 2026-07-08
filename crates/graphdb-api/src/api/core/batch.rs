//! Batch Operation API - Core Layer
//!
//! Provides transport layer-independent batch operation capabilities
//! Supports both synchronous and asynchronous execution modes

use crate::api::core::CoreResult;
use crate::core::{Edge, Vertex};
use crate::storage::StorageClient;

/// Batch operation configuration
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Batch size for auto-flush
    pub batch_size: usize,
    /// Whether to auto-flush when buffer is full
    pub auto_flush: bool,
    /// Whether to continue on error
    pub continue_on_error: bool,
    /// Whether to auto-commit (alias for auto_flush for API compatibility)
    pub auto_commit: bool,
    /// Maximum number of errors before stopping (None means unlimited)
    pub max_errors: Option<usize>,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            auto_flush: true,
            continue_on_error: true,
            auto_commit: true,
            max_errors: Some(100),
        }
    }
}

impl BatchConfig {
    /// Create default configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set batch size
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    /// Set auto-flush
    pub fn with_auto_flush(mut self, auto_flush: bool) -> Self {
        self.auto_flush = auto_flush;
        self
    }

    /// Set continue on error
    pub fn with_continue_on_error(mut self, continue_on_error: bool) -> Self {
        self.continue_on_error = continue_on_error;
        self
    }

    /// Set auto-commit (alias for auto_flush)
    pub fn with_auto_commit(mut self, auto_commit: bool) -> Self {
        self.auto_commit = auto_commit;
        self.auto_flush = auto_commit;
        self
    }

    /// Set max errors
    pub fn with_max_errors(mut self, max_errors: Option<usize>) -> Self {
        self.max_errors = max_errors;
        self
    }
}

/// Batch item type
#[derive(Debug, Clone)]
pub enum BatchItem {
    /// Vertex to insert
    Vertex(Vertex),
    /// Edge to insert
    Edge(Edge),
}

/// Batch operation result
#[derive(Debug, Clone, Default)]
pub struct BatchResult {
    /// Number of vertices inserted
    pub vertices_inserted: usize,
    /// Number of edges inserted
    pub edges_inserted: usize,
    /// Number of failed operations
    pub failed_count: usize,
    /// Error messages for failed operations
    pub errors: Vec<BatchError>,
}

/// Batch error
#[derive(Debug, Clone)]
pub struct BatchError {
    /// Index in the batch
    pub index: usize,
    /// Item type
    pub item_type: BatchItemType,
    /// Error message
    pub message: String,
}

/// Batch item type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchItemType {
    /// Vertex
    Vertex,
    /// Edge
    Edge,
}

/// Core batch operation
///
/// This struct provides the core batch operation logic that can be used
/// by both embedded and server layers.
#[derive(Debug)]
pub struct BatchOperation {
    items: Vec<BatchItem>,
    config: BatchConfig,
}

impl BatchOperation {
    /// Create a new batch operation
    pub fn new(config: BatchConfig) -> Self {
        Self {
            items: Vec::with_capacity(config.batch_size),
            config,
        }
    }

    /// Add a vertex to the batch
    pub fn add_vertex(&mut self, vertex: Vertex) {
        self.items.push(BatchItem::Vertex(vertex));
    }

    /// Add an edge to the batch
    pub fn add_edge(&mut self, edge: Edge) {
        self.items.push(BatchItem::Edge(edge));
    }

    /// Add multiple items to the batch
    pub fn add_items(&mut self, items: Vec<BatchItem>) {
        self.items.extend(items);
    }

    /// Get current item count
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Check if batch is empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Check if batch should be flushed
    pub fn should_flush(&self) -> bool {
        self.config.auto_flush && self.items.len() >= self.config.batch_size
    }

    /// Get batch size
    pub fn batch_size(&self) -> usize {
        self.config.batch_size
    }

    /// Clear all items
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Take all items (clears internal buffer)
    pub fn take_items(&mut self) -> Vec<BatchItem> {
        std::mem::take(&mut self.items)
    }

    /// Execute batch operation synchronously
    ///
    /// # Parameters
    /// - `storage`: storage client
    /// - `space_name`: graph space name
    ///
    /// # Returns
    /// Batch operation result
    pub fn execute_sync<S: StorageClient>(
        &mut self,
        storage: &mut S,
        space_name: &str,
    ) -> CoreResult<BatchResult> {
        let items = self.take_items();
        Self::execute_items_sync(storage, space_name, items, &self.config)
    }

    /// Execute batch items synchronously
    fn execute_items_sync<S: StorageClient>(
        storage: &mut S,
        space_name: &str,
        items: Vec<BatchItem>,
        config: &BatchConfig,
    ) -> CoreResult<BatchResult> {
        let mut result = BatchResult::default();
        let mut vertices = Vec::new();
        let mut edges = Vec::new();

        // Separate vertices and edges
        for item in items {
            match item {
                BatchItem::Vertex(v) => vertices.push(v),
                BatchItem::Edge(e) => edges.push(e),
            }
        }

        // Insert vertices
        if !vertices.is_empty() {
            let vertex_count = vertices.len();
            match storage.batch_insert_vertices(space_name, vertices) {
                Ok(_) => {
                    result.vertices_inserted = vertex_count;
                }
                Err(e) => {
                    let error = BatchError {
                        index: 0,
                        item_type: BatchItemType::Vertex,
                        message: format!("Failed to insert vertices: {}", e),
                    };
                    result.errors.push(error);
                    result.failed_count += 1;
                    if !config.continue_on_error {
                        return Ok(result);
                    }
                }
            }
        }

        // Insert edges
        if !edges.is_empty() {
            let edge_count = edges.len();
            match storage.batch_insert_edges(space_name, edges) {
                Ok(()) => {
                    result.edges_inserted = edge_count;
                }
                Err(e) => {
                    let error = BatchError {
                        index: 0,
                        item_type: BatchItemType::Edge,
                        message: format!("Failed to insert edges: {}", e),
                    };
                    result.errors.push(error);
                    result.failed_count += 1;
                }
            }
        }

        Ok(result)
    }
}

/// Batch operation builder
#[derive(Debug)]
pub struct BatchOperationBuilder {
    config: BatchConfig,
}

impl BatchOperationBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            config: BatchConfig::default(),
        }
    }

    /// Set batch size
    pub fn batch_size(mut self, size: usize) -> Self {
        self.config.batch_size = size;
        self
    }

    /// Set auto-flush
    pub fn auto_flush(mut self, auto_flush: bool) -> Self {
        self.config.auto_flush = auto_flush;
        self
    }

    /// Set continue on error
    pub fn continue_on_error(mut self, continue_on_error: bool) -> Self {
        self.config.continue_on_error = continue_on_error;
        self
    }

    /// Build batch operation
    pub fn build(self) -> BatchOperation {
        BatchOperation::new(self.config)
    }
}

impl Default for BatchOperationBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_batch_config_default() {
        let config = BatchConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert!(config.auto_flush);
        assert!(config.continue_on_error);
    }

    #[test]
    fn test_batch_config_builder() {
        let config = BatchConfig::new()
            .with_batch_size(500)
            .with_auto_flush(false)
            .with_continue_on_error(true);

        assert_eq!(config.batch_size, 500);
        assert!(!config.auto_flush);
        assert!(config.continue_on_error);
    }

    #[test]
    fn test_batch_operation_add_items() {
        let mut batch = BatchOperation::new(BatchConfig::default());

        let vertex = Vertex::with_vid(crate::core::types::VertexId::from_int64(1));
        batch.add_vertex(vertex);

        assert_eq!(batch.len(), 1);
        assert!(!batch.is_empty());
    }

    #[test]
    fn test_batch_operation_should_flush() {
        let config = BatchConfig::new().with_batch_size(2);
        let mut batch = BatchOperation::new(config);

        assert!(!batch.should_flush());

        let vertex = Vertex::with_vid(crate::core::types::VertexId::from_int64(1));
        batch.add_vertex(vertex.clone());
        batch.add_vertex(vertex);

        assert!(batch.should_flush());
    }
}
