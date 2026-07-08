//! Insert Operation Plan Nodes
//!
//! Provides plan nodes for INSERT VERTEX and INSERT EDGE operations.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::define_plan_node;

use super::info::{EdgeInsertInfo, TagInsertSpec, VertexInsertInfo};

define_plan_node! {
    pub struct InsertVerticesNode {
        info: VertexInsertInfo,
    }
    enum: InsertVertices
    input: ZeroInputNode
}

impl InsertVerticesNode {
    pub fn new(id: i64, info: VertexInsertInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: vec!["inserted".to_string()],
        }
    }

    pub fn info(&self) -> &VertexInsertInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    /// Get all tag names
    pub fn tag_names(&self) -> Vec<String> {
        self.info.tags.iter().map(|t| t.tag_name.clone()).collect()
    }

    /// Get the first tag name (for backward compatibility)
    pub fn tag_name(&self) -> Option<&str> {
        self.info.tags.first().map(|t| t.tag_name.as_str())
    }

    /// Get all tag specifications
    pub fn tags(&self) -> &[TagInsertSpec] {
        &self.info.tags
    }

    /// Get property names of the first tag (for backward compatibility)
    pub fn prop_names(&self) -> Option<&[String]> {
        self.info.tags.first().map(|t| t.prop_names.as_slice())
    }

    /// Get all values
    pub fn values(&self) -> &[(ContextualExpression, Vec<Vec<ContextualExpression>>)] {
        &self.info.values
    }

    /// Get IF NOT EXISTS flag
    pub fn if_not_exists(&self) -> bool {
        self.info.if_not_exists
    }
}

define_plan_node! {
    pub struct InsertEdgesNode {
        info: EdgeInsertInfo,
    }
    enum: InsertEdges
    input: ZeroInputNode
}

impl InsertEdgesNode {
    pub fn new(id: i64, info: EdgeInsertInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: vec!["inserted".to_string()],
        }
    }

    pub fn info(&self) -> &EdgeInsertInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn edge_name(&self) -> &str {
        &self.info.edge_name
    }

    pub fn prop_names(&self) -> &[String] {
        &self.info.prop_names
    }

    pub fn edges(
        &self,
    ) -> &[(
        ContextualExpression,
        ContextualExpression,
        Option<ContextualExpression>,
        Vec<ContextualExpression>,
    )] {
        &self.info.edges
    }

    /// Get IF NOT EXISTS flag
    pub fn if_not_exists(&self) -> bool {
        self.info.if_not_exists
    }
}
