//! Batch import contract DTOs (HTTP `/batch` endpoints).
//!
//! Previously mirrored between `api/server/batch/types.rs` and
//! `cli/client/{batch,request_types,response_types}.rs`. `BatchItem` uses
//! internally-tagged representation (`{"type": "vertex", ...}`), the shape
//! the CLI already emitted.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;

/// Batch Task ID
pub type BatchId = String;

/// Batch Task Status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStatus {
    /// Created
    Created,
    /// Running
    Running,
    /// Completed
    Completed,
    /// Failed
    Failed,
    /// Cancelled
    Cancelled,
}

impl fmt::Display for BatchStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            BatchStatus::Created => "Created",
            BatchStatus::Running => "Running",
            BatchStatus::Completed => "Completed",
            BatchStatus::Failed => "Failed",
            BatchStatus::Cancelled => "Cancelled",
        };
        write!(f, "{}", name)
    }
}

/// Batch task type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchType {
    /// Vertex batch insertion
    Vertex,
    /// Edge batch insertion
    Edge,
    /// Mixed batch insertion
    Mixed,
}

impl fmt::Display for BatchType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            BatchType::Vertex => "vertex",
            BatchType::Edge => "edge",
            BatchType::Mixed => "mixed",
        };
        write!(f, "{}", name)
    }
}

/// Batch item type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchItemType {
    /// Vertex item
    Vertex,
    /// Edge item
    Edge,
}

impl fmt::Display for BatchItemType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            BatchItemType::Vertex => "vertex",
            BatchItemType::Edge => "edge",
        };
        write!(f, "{}", name)
    }
}

/// Batch data
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BatchItem {
    #[serde(rename = "vertex")]
    Vertex(VertexData),
    #[serde(rename = "edge")]
    Edge(EdgeData),
}

/// Vertex data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertexData {
    /// Vertex ID
    pub vid: serde_json::Value,
    /// Tag list
    #[serde(default)]
    pub tags: Vec<String>,
    /// Properties
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
}

/// Edge data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeData {
    /// Edge type
    pub edge_type: String,
    /// Source vertex ID
    pub src_vid: serde_json::Value,
    /// Target vertex ID
    pub dst_vid: serde_json::Value,
    /// Properties
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
}

/// Create batch task request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBatchRequest {
    /// Space ID
    pub space_id: u64,
    /// Batch task type
    pub batch_type: BatchType,
    /// Batch size
    #[serde(default = "default_batch_size")]
    pub batch_size: usize,
}

fn default_batch_size() -> usize {
    1000
}

/// Create batch task response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateBatchResponse {
    /// Batch task ID
    pub batch_id: BatchId,
    /// Task status
    pub status: BatchStatus,
    /// Creation time
    pub created_at: String,
}

/// Add batch items request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddBatchItemsRequest {
    /// Batch item list
    pub items: Vec<BatchItem>,
}

/// Add batch items response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddBatchItemsResponse {
    /// Number accepted
    pub accepted: usize,
    /// Number buffered
    #[serde(default)]
    pub buffered: usize,
    /// Total number buffered
    #[serde(default)]
    pub total_buffered: usize,
}

/// Execute batch task response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecuteBatchResponse {
    /// Batch task ID
    pub batch_id: BatchId,
    /// Task status
    pub status: BatchStatus,
    /// Execution result
    pub result: BatchResultData,
    /// Completion time
    #[serde(default)]
    pub completed_at: Option<String>,
}

/// Batch results data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchResultData {
    /// Number of vertices inserted
    pub vertices_inserted: usize,
    /// Number of edges inserted
    pub edges_inserted: usize,
    /// Error message
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<BatchErrorData>,
}

/// Batch error data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchErrorData {
    /// Index where the error occurred
    pub index: usize,
    /// Type of the item
    pub item_type: BatchItemType,
    /// Error message
    pub error: String,
}

/// Batch task status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchStatusResponse {
    /// Batch task ID
    pub batch_id: BatchId,
    /// Task status
    pub status: BatchStatus,
    /// Progress information
    pub progress: BatchProgress,
    /// Creation time
    pub created_at: String,
    /// Update time
    pub updated_at: String,
}

/// Progress of batch tasks
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchProgress {
    /// Total quantity
    pub total: usize,
    /// Number of items processed
    pub processed: usize,
    /// Number of successes
    pub succeeded: usize,
    /// Number of failures
    pub failed: usize,
    /// Number buffered
    #[serde(default)]
    pub buffered: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_item_vertex_roundtrip() {
        let item = BatchItem::Vertex(VertexData {
            vid: serde_json::json!("v1"),
            tags: vec!["person".to_string()],
            properties: HashMap::from([("name".to_string(), serde_json::json!("Alice"))]),
        });
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains(r#""type":"vertex""#), "json: {json}");
        let back: BatchItem = serde_json::from_str(&json).unwrap();
        match back {
            BatchItem::Vertex(v) => {
                assert_eq!(v.vid, serde_json::json!("v1"));
                assert_eq!(v.tags, vec!["person"]);
                assert_eq!(v.properties.get("name"), Some(&serde_json::json!("Alice")));
            }
            other => panic!("expected vertex item, got {other:?}"),
        }
    }

    #[test]
    fn batch_item_edge_roundtrip() {
        let item = BatchItem::Edge(EdgeData {
            edge_type: "follows".to_string(),
            src_vid: serde_json::json!("v1"),
            dst_vid: serde_json::json!("v2"),
            properties: HashMap::new(),
        });
        let json = serde_json::to_string(&item).unwrap();
        let back: BatchItem = serde_json::from_str(&json).unwrap();
        match back {
            BatchItem::Edge(e) => assert_eq!(e.edge_type, "follows"),
            other => panic!("expected edge item, got {other:?}"),
        }
    }

    #[test]
    fn batch_status_wire_names() {
        assert_eq!(
            serde_json::to_string(&BatchStatus::Completed).unwrap(),
            r#""completed""#
        );
        assert_eq!(
            serde_json::from_str::<BatchStatus>(r#""running""#).unwrap(),
            BatchStatus::Running
        );
        assert_eq!(
            serde_json::from_str::<BatchType>(r#""mixed""#).unwrap(),
            BatchType::Mixed
        );
    }

    #[test]
    fn create_batch_request_roundtrip() {
        let request = CreateBatchRequest {
            space_id: 1,
            batch_type: BatchType::Vertex,
            batch_size: 100,
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: CreateBatchRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.space_id, 1);
        assert_eq!(back.batch_type, BatchType::Vertex);
        assert_eq!(back.batch_size, 100);
    }

    #[test]
    fn execute_batch_response_roundtrip() {
        let response = ExecuteBatchResponse {
            batch_id: "b1".to_string(),
            status: BatchStatus::Completed,
            result: BatchResultData {
                vertices_inserted: 2,
                edges_inserted: 0,
                errors: Vec::new(),
            },
            completed_at: Some("now".to_string()),
        };
        let json = serde_json::to_string(&response).unwrap();
        let back: ExecuteBatchResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.result.vertices_inserted, 2);
        assert_eq!(back.status, BatchStatus::Completed);
    }
}
