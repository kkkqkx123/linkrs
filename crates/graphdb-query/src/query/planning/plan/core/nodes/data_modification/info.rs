//! Data Modification Info Structures
//!
//! Provides shared information structures for INSERT, UPDATE, DELETE operations.

use crate::core::types::expr::contextual::ContextualExpression;
use std::collections::HashMap;

// ==================== INSERT Info Structures ====================

/// Tag insertion specification for INSERT operations
#[derive(Debug, Clone)]
pub struct TagInsertSpec {
    pub tag_name: String,
    pub prop_names: Vec<String>,
}

/// Vertex insertion information
#[derive(Debug, Clone)]
pub struct VertexInsertInfo {
    pub space_name: String,
    pub tags: Vec<TagInsertSpec>,
    /// (vertex_id, tag_values) pairs
    pub values: Vec<(ContextualExpression, Vec<Vec<ContextualExpression>>)>,
    /// IF NOT EXISTS flag
    pub if_not_exists: bool,
}

/// Edge insertion information
#[derive(Debug, Clone)]
pub struct EdgeInsertInfo {
    pub space_name: String,
    pub edge_name: String,
    pub prop_names: Vec<String>,
    /// (src, dst, rank, prop_values) tuples
    pub edges: Vec<(
        ContextualExpression,
        ContextualExpression,
        Option<ContextualExpression>,
        Vec<ContextualExpression>,
    )>,
    /// IF NOT EXISTS flag
    pub if_not_exists: bool,
}

// ==================== UPDATE Info Structures ====================

/// Vertex update information
#[derive(Debug, Clone)]
pub struct VertexUpdateInfo {
    pub space_name: String,
    pub vertex_id: ContextualExpression,
    pub tag_name: Option<String>,
    pub properties: HashMap<String, ContextualExpression>,
    pub condition: Option<ContextualExpression>,
    pub is_upsert: bool,
}

/// Edge update information
#[derive(Debug, Clone)]
pub struct EdgeUpdateInfo {
    pub space_name: String,
    pub src: ContextualExpression,
    pub dst: ContextualExpression,
    pub edge_type: Option<String>,
    pub rank: Option<ContextualExpression>,
    pub properties: HashMap<String, ContextualExpression>,
    pub condition: Option<ContextualExpression>,
    pub is_upsert: bool,
}

/// Update target type enum
#[derive(Debug, Clone)]
pub enum UpdateTargetType {
    Vertex(VertexUpdateInfo),
    Edge(EdgeUpdateInfo),
}

// ==================== DELETE Info Structures ====================

/// Vertex deletion information
#[derive(Debug, Clone)]
pub struct VertexDeleteInfo {
    pub space_name: String,
    pub vertex_ids: Vec<ContextualExpression>,
    pub with_edge: bool,
    pub condition: Option<ContextualExpression>,
}

/// Edge deletion information
#[derive(Debug, Clone)]
pub struct EdgeDeleteInfo {
    pub space_name: String,
    pub edge_type: Option<String>,
    /// (src, dst, rank) tuples
    pub edges: Vec<(
        ContextualExpression,
        ContextualExpression,
        Option<ContextualExpression>,
    )>,
    pub condition: Option<ContextualExpression>,
}

/// Tag deletion information
#[derive(Debug, Clone)]
pub struct TagDeleteInfo {
    pub space_name: String,
    pub tag_names: Vec<String>,
    pub vertex_ids: Vec<ContextualExpression>,
    pub is_all_tags: bool,
}

/// Index deletion information
#[derive(Debug, Clone)]
pub struct IndexDeleteInfo {
    pub space_name: String,
    pub index_name: String,
}
