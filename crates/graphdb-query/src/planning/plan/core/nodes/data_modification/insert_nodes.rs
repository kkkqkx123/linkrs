//! Insert Operation Plan Nodes
//!
//! Provides plan nodes for INSERT VERTEX and INSERT EDGE operations.

use crate::define_plan_node;
use graphdb_core::types::expr::contextual::ContextualExpression;

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
            column_types: vec![],
        }
    }

    pub fn info(&self) -> &VertexInsertInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    /// Get the tag name
    pub fn tag_name(&self) -> &str {
        self.info.tag.tag_name.as_str()
    }

    /// Get the tag specification
    pub fn tag(&self) -> &TagInsertSpec {
        &self.info.tag
    }

    /// Get property names of the tag
    pub fn prop_names(&self) -> &[String] {
        self.info.tag.prop_names.as_slice()
    }

    /// Get all values
    pub fn values(&self) -> &[(ContextualExpression, Vec<ContextualExpression>)] {
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
            column_types: vec![],
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
