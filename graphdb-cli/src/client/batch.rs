//! Batch operation types

use std::collections::HashMap;

/// Batch operation types
#[derive(Debug, Clone)]
pub enum BatchType {
    Vertex,
    Edge,
    Mixed,
}

/// Batch item for bulk operations
#[derive(Debug, Clone)]
pub enum BatchItem {
    Vertex(VertexData),
    Edge(EdgeData),
}

/// Vertex data for batch insertion
#[derive(Debug, Clone)]
pub struct VertexData {
    pub vid: serde_json::Value,
    pub tags: Vec<String>,
    pub properties: HashMap<String, serde_json::Value>,
}

/// Edge data for batch insertion
#[derive(Debug, Clone)]
pub struct EdgeData {
    pub edge_type: String,
    pub src_vid: serde_json::Value,
    pub dst_vid: serde_json::Value,
    pub properties: HashMap<String, serde_json::Value>,
}

/// Batch operation result
#[derive(Debug, Clone)]
pub struct BatchResult {
    pub batch_id: String,
    pub status: String,
    pub vertices_inserted: usize,
    pub edges_inserted: usize,
    pub errors: Vec<BatchError>,
}

/// Batch error information
#[derive(Debug, Clone)]
pub struct BatchError {
    pub index: usize,
    pub item_type: String,
    pub error: String,
}

/// Batch status information
#[derive(Debug, Clone)]
pub struct BatchStatus {
    pub batch_id: String,
    pub status: String,
    pub total: usize,
    pub processed: usize,
    pub succeeded: usize,
    pub failed: usize,
}
