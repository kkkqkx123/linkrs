//! Bulk Insert API - Core Layer
//!
//! Provides transport layer-independent bulk insert capabilities.
//! The buffering operation is shared by the embedded layer and the network
//! service layer.
//!
//! Contract: this is a data-level write path. Items go straight to
//! `storage.batch_insert_vertices` / `batch_insert_edges`, bypassing the
//! statement pipeline and the `TransactionManager`; secondary-index
//! synchronization is handled by the storage SyncWrapper. Statement-level
//! batch execution belongs to `QueryApi::execute_batch` and must not be
//! mixed with this path.

use linkrs_storage::StorageClient;
use crate::CoreResult;
use linkrs_core::{Edge, Vertex};

/// Bulk insert configuration
#[derive(Debug, Clone)]
pub struct BulkInsertConfig {
    /// Number of buffered items before an automatic flush
    pub batch_size: usize,
    /// Whether to flush automatically when the buffer is full
    pub auto_flush: bool,
    /// Whether to continue after a failed chunk
    pub continue_on_error: bool,
    /// Maximum number of recorded errors (None means unlimited)
    pub max_errors: Option<usize>,
}

impl Default for BulkInsertConfig {
    fn default() -> Self {
        Self {
            batch_size: 1000,
            auto_flush: true,
            continue_on_error: true,
            max_errors: Some(100),
        }
    }
}

impl BulkInsertConfig {
    /// Create default configuration
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the buffer size that triggers a flush
    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size.max(1);
        self
    }

    /// Set automatic flushing
    pub fn with_auto_flush(mut self, auto_flush: bool) -> Self {
        self.auto_flush = auto_flush;
        self
    }

    /// Set continue-on-error
    pub fn with_continue_on_error(mut self, continue_on_error: bool) -> Self {
        self.continue_on_error = continue_on_error;
        self
    }

    /// Set the maximum number of recorded errors
    pub fn with_max_errors(mut self, max_errors: Option<usize>) -> Self {
        self.max_errors = max_errors;
        self
    }
}

/// Item buffered for one bulk insert
#[derive(Debug, Clone)]
pub enum BulkInsertItem {
    /// Vertex to insert
    Vertex(Vertex),
    /// Edge to insert
    Edge(Edge),
}

/// Bulk insert result
#[derive(Debug, Clone, Default)]
pub struct BulkInsertResult {
    /// Number of vertices inserted
    pub vertices_inserted: usize,
    /// Number of edges inserted
    pub edges_inserted: usize,
    /// Number of failed chunks
    pub failed_count: usize,
    /// Errors recorded for failed chunks
    pub errors: Vec<BulkInsertError>,
}

/// Error recorded for one failed chunk of a bulk insert
#[derive(Debug, Clone)]
pub struct BulkInsertError {
    /// Chunk index inside the bulk insert
    pub index: usize,
    /// Chunk item type
    pub item_type: BulkInsertItemType,
    /// Error message
    pub message: String,
}

/// Chunk item type of a bulk insert
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkInsertItemType {
    /// Vertex chunk
    Vertex,
    /// Edge chunk
    Edge,
}

/// Core bulk insert operation
///
/// Holds the buffered items and maps them onto the storage batch insert
/// entry points. Used by both the embedded layer and the network service
/// layer.
#[derive(Debug)]
pub struct BulkInsertOperation {
    items: Vec<BulkInsertItem>,
    config: BulkInsertConfig,
}

impl BulkInsertOperation {
    /// Create a new bulk insert operation
    pub fn new(config: BulkInsertConfig) -> Self {
        Self {
            items: Vec::with_capacity(config.batch_size),
            config,
        }
    }

    /// Buffer a vertex
    pub fn add_vertex(&mut self, vertex: Vertex) {
        self.items.push(BulkInsertItem::Vertex(vertex));
    }

    /// Buffer an edge
    pub fn add_edge(&mut self, edge: Edge) {
        self.items.push(BulkInsertItem::Edge(edge));
    }

    /// Buffer multiple items
    pub fn add_items(&mut self, items: Vec<BulkInsertItem>) {
        self.items.extend(items);
    }

    /// Number of buffered items
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Whether the buffer reached the configured flush size
    pub fn should_flush(&self) -> bool {
        self.config.auto_flush && self.items.len() >= self.config.batch_size
    }

    /// Configured flush size
    pub fn batch_size(&self) -> usize {
        self.config.batch_size
    }

    /// Drop all buffered items
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Take all buffered items, leaving the buffer empty
    pub fn take_items(&mut self) -> Vec<BulkInsertItem> {
        std::mem::take(&mut self.items)
    }

    /// Insert every buffered item through one storage call per item kind
    ///
    /// # Parameters
    /// - `storage`: storage client
    /// - `space_name`: graph space name
    ///
    /// # Returns
    /// Bulk insert result; per-chunk failures are recorded in the result
    /// instead of failing the whole call.
    pub fn execute_sync<S: StorageClient>(
        &mut self,
        storage: &mut S,
        space_name: &str,
    ) -> CoreResult<BulkInsertResult> {
        let items = self.take_items();
        Self::execute_items_sync(storage, space_name, items, &self.config)
    }

    fn execute_items_sync<S: StorageClient>(
        storage: &mut S,
        space_name: &str,
        items: Vec<BulkInsertItem>,
        config: &BulkInsertConfig,
    ) -> CoreResult<BulkInsertResult> {
        let mut result = BulkInsertResult::default();
        let mut vertices = Vec::new();
        let mut edges = Vec::new();

        for item in items {
            match item {
                BulkInsertItem::Vertex(v) => vertices.push(v),
                BulkInsertItem::Edge(e) => edges.push(e),
            }
        }

        if !vertices.is_empty() {
            let vertex_count = vertices.len();
            match storage.batch_insert_vertices(space_name, vertices) {
                Ok(_) => {
                    result.vertices_inserted = vertex_count;
                }
                Err(e) => {
                    let error = BulkInsertError {
                        index: 0,
                        item_type: BulkInsertItemType::Vertex,
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

        if !edges.is_empty() {
            let edge_count = edges.len();
            match storage.batch_insert_edges(space_name, edges) {
                Ok(()) => {
                    result.edges_inserted = edge_count;
                }
                Err(e) => {
                    let error = BulkInsertError {
                        index: 0,
                        item_type: BulkInsertItemType::Edge,
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

/// Builder for [`BulkInsertOperation`]
#[derive(Debug)]
pub struct BulkInsertOperationBuilder {
    config: BulkInsertConfig,
}

impl BulkInsertOperationBuilder {
    /// Create a new builder
    pub fn new() -> Self {
        Self {
            config: BulkInsertConfig::default(),
        }
    }

    /// Set the buffer size that triggers a flush
    pub fn batch_size(mut self, size: usize) -> Self {
        self.config.batch_size = size;
        self
    }

    /// Set automatic flushing
    pub fn auto_flush(mut self, auto_flush: bool) -> Self {
        self.config.auto_flush = auto_flush;
        self
    }

    /// Set continue-on-error
    pub fn continue_on_error(mut self, continue_on_error: bool) -> Self {
        self.config.continue_on_error = continue_on_error;
        self
    }

    /// Build the bulk insert operation
    pub fn build(self) -> BulkInsertOperation {
        BulkInsertOperation::new(self.config)
    }
}

impl Default for BulkInsertOperationBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bulk_insert_config_default() {
        let config = BulkInsertConfig::default();
        assert_eq!(config.batch_size, 1000);
        assert!(config.auto_flush);
        assert!(config.continue_on_error);
    }

    #[test]
    fn bulk_insert_config_builder() {
        let config = BulkInsertConfig::new()
            .with_batch_size(500)
            .with_auto_flush(false)
            .with_continue_on_error(true);

        assert_eq!(config.batch_size, 500);
        assert!(!config.auto_flush);
        assert!(config.continue_on_error);
    }

    #[test]
    fn bulk_insert_operation_add_items() {
        let mut operation = BulkInsertOperation::new(BulkInsertConfig::default());

        let vertex = Vertex::new(
            linkrs_core::types::VertexId::try_from_int64(1).expect("test vertex id"),
            linkrs_core::Tag::new("test".into(), std::collections::HashMap::new()),
        );
        operation.add_vertex(vertex);

        assert_eq!(operation.len(), 1);
        assert!(!operation.is_empty());
    }

    #[test]
    fn bulk_insert_operation_should_flush() {
        let config = BulkInsertConfig::new().with_batch_size(2);
        let mut operation = BulkInsertOperation::new(config);

        assert!(!operation.should_flush());

        let vertex = Vertex::new(
            linkrs_core::types::VertexId::try_from_int64(1).expect("test vertex id"),
            linkrs_core::Tag::new("test".into(), std::collections::HashMap::new()),
        );
        operation.add_vertex(vertex.clone());
        operation.add_vertex(vertex);

        assert!(operation.should_flush());
    }
}
