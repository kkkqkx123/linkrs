//! WAL Redo Log Types
//!
//! Redo log entry types for WAL replay during recovery.
//! All vertex references use unified VertexId (supports both int64 and string IDs).

use serde::{Deserialize, Serialize};

use crate::types::{LabelId, SpaceInfo, VertexId};
use crate::Value;

// ============================================================================
// Data Operations
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertVertexRedo {
    pub label: LabelId,
    pub vid: VertexId,
    pub properties: Vec<(String, Value)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsertEdgeRedo {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub properties: Vec<(String, Value)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateVertexPropRedo {
    pub label: LabelId,
    pub vid: VertexId,
    pub prop_name: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateEdgePropRedo {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub prop_name: String,
    pub value: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteVertexRedo {
    pub label: LabelId,
    pub vid: VertexId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteEdgeRedo {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
}

// ============================================================================
// Schema Operations
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSpaceRedo {
    pub space: SpaceInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropSpaceRedo {
    pub space_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearSpaceRedo {
    pub space_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlterSpaceCommentRedo {
    pub space_id: u64,
    pub comment: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVertexTypeRedo {
    pub space_name: String,
    pub label_id: Option<LabelId>,
    pub label_name: String,
    /// (property name, type name, is_serial) triples.
    pub schema: Vec<(String, String, bool)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEdgeTypeRedo {
    pub space_name: String,
    pub label_id: Option<LabelId>,
    pub src_label: String,
    pub dst_label: String,
    pub edge_label: String,
    /// (property name, type name, is_serial) triples.
    pub schema: Vec<(String, String, bool)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteVertexTypeRedo {
    pub space_name: Option<String>,
    pub label_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteEdgeTypeRedo {
    pub space_name: Option<String>,
    pub src_label: String,
    pub dst_label: String,
    pub edge_label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddVertexPropRedo {
    pub label: LabelId,
    /// (property name, type name, is_serial) triples.
    pub properties: Vec<(String, String, bool)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddEdgePropRedo {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
    /// (property name, type name, is_serial) triples.
    pub properties: Vec<(String, String, bool)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteVertexPropRedo {
    pub label: LabelId,
    pub prop_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteEdgePropRedo {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
    pub prop_names: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameVertexPropRedo {
    pub label: LabelId,
    pub old_name: String,
    pub new_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenameEdgePropRedo {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
    pub old_name: String,
    pub new_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTagIndexRedo {
    pub space_id: u64,
    pub index_name: String,
    pub fields: Vec<(String, String)>,
    pub properties: Vec<String>,
    pub is_unique: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropTagIndexRedo {
    pub space_id: u64,
    pub index_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateEdgeIndexRedo {
    pub space_id: u64,
    pub index_name: String,
    pub fields: Vec<(String, String)>,
    pub properties: Vec<String>,
    pub is_unique: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropEdgeIndexRedo {
    pub space_id: u64,
    pub index_name: String,
}

// Compact has no redo data (just timestamp in the WAL header).
// A struct exists for completeness but carries no payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactRedo;
