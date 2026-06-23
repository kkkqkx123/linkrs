//! Delete Operation Plan Nodes
//!
//! Provides plan nodes for DELETE VERTEX and DELETE EDGE operations.
//! - DeleteVerticesNode/DeleteEdgesNode: ZeroInputNode for standalone DELETE
//! - PipeDeleteVerticesNode/PipeDeleteEdgesNode: SingleInputNode for pipe-based DELETE

use crate::core::types::expr::contextual::ContextualExpression;
use crate::{define_plan_node, define_plan_node_with_deps};

use super::info::{EdgeDeleteInfo, IndexDeleteInfo, TagDeleteInfo, VertexDeleteInfo};

// ============================================================================
// ZeroInputNode: Standalone DELETE (no input from pipe)
// ============================================================================

define_plan_node! {
    /// Delete vertices node (standalone)
    ///
    /// Used for: DELETE VERTEX "vid1", "vid2"
    pub struct DeleteVerticesNode {
        info: VertexDeleteInfo,
    }
    enum: DeleteVertices
    input: ZeroInputNode
}

impl DeleteVerticesNode {
    pub fn new(id: i64, info: VertexDeleteInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: vec!["deleted".to_string()],
        }
    }

    pub fn info(&self) -> &VertexDeleteInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn vertex_ids(&self) -> &[ContextualExpression] {
        &self.info.vertex_ids
    }

    pub fn with_edge(&self) -> bool {
        self.info.with_edge
    }

    pub fn condition(&self) -> Option<&ContextualExpression> {
        self.info.condition.as_ref()
    }
}

// ============================================================================
// ZeroInputNode: DELETE TAG / DELETE TAG *
// ============================================================================

define_plan_node! {
    /// Delete tags node (standalone)
    ///
    /// Used for: DELETE TAG tag1, tag2 FROM "vid1", "vid2"
    ///           DELETE TAG * FROM "vid1", "vid2"
    pub struct DeleteTagsNode {
        info: TagDeleteInfo,
    }
    enum: DeleteTags
    input: ZeroInputNode
}

impl DeleteTagsNode {
    pub fn new(id: i64, info: TagDeleteInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: vec!["deleted".to_string()],
        }
    }

    pub fn info(&self) -> &TagDeleteInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn tag_names(&self) -> &[String] {
        &self.info.tag_names
    }

    pub fn vertex_ids(&self) -> &[ContextualExpression] {
        &self.info.vertex_ids
    }

    pub fn is_all_tags(&self) -> bool {
        self.info.is_all_tags
    }
}

// ============================================================================
// ZeroInputNode: DELETE INDEX
// ============================================================================

define_plan_node! {
    /// Delete index node (standalone)
    ///
    /// Used for: DELETE INDEX index_name
    pub struct DeleteIndexNode {
        info: IndexDeleteInfo,
    }
    enum: DeleteIndex
    input: ZeroInputNode
}

impl DeleteIndexNode {
    pub fn new(id: i64, info: IndexDeleteInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: vec!["deleted".to_string()],
        }
    }

    pub fn info(&self) -> &IndexDeleteInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn index_name(&self) -> &str {
        &self.info.index_name
    }
}

define_plan_node! {
    /// Delete edges node (standalone)
    ///
    /// Used for: DELETE EDGE edge_type "src" -> "dst"
    pub struct DeleteEdgesNode {
        info: EdgeDeleteInfo,
    }
    enum: DeleteEdges
    input: ZeroInputNode
}

impl DeleteEdgesNode {
    pub fn new(id: i64, info: EdgeDeleteInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: vec!["deleted".to_string()],
        }
    }

    pub fn info(&self) -> &EdgeDeleteInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn edge_type(&self) -> Option<&str> {
        self.info.edge_type.as_deref()
    }

    pub fn edges(
        &self,
    ) -> &[(
        ContextualExpression,
        ContextualExpression,
        Option<ContextualExpression>,
    )] {
        &self.info.edges
    }

    pub fn condition(&self) -> Option<&ContextualExpression> {
        self.info.condition.as_ref()
    }
}

// ============================================================================
// SingleInputNode: Pipe-based DELETE (receives input from pipe)
// ============================================================================

define_plan_node_with_deps! {
    /// Pipe delete vertices node
    ///
    /// Used for: GO FROM "vid" OVER edge YIELD dst(edge) AS id | DELETE VERTEX $-.id
    pub struct PipeDeleteVerticesNode {
        info: VertexDeleteInfo,
    }
    enum: PipeDeleteVertices
    input: SingleInputNode
}

impl PipeDeleteVerticesNode {
    pub fn new(
        id: i64,
        info: VertexDeleteInfo,
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) -> Self {
        Self {
            id,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            info,
            output_var: None,
            col_names: vec!["deleted".to_string()],
        }
    }

    pub fn info(&self) -> &VertexDeleteInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn vertex_ids(&self) -> &[ContextualExpression] {
        &self.info.vertex_ids
    }

    pub fn with_edge(&self) -> bool {
        self.info.with_edge
    }

    pub fn condition(&self) -> Option<&ContextualExpression> {
        self.info.condition.as_ref()
    }
}

define_plan_node_with_deps! {
    /// Pipe delete edges node
    ///
    /// Used for: GO FROM "vid" OVER edge YIELD src(edge) AS s, dst(edge) AS d | DELETE EDGE type $-.s -> $-.d
    pub struct PipeDeleteEdgesNode {
        info: EdgeDeleteInfo,
    }
    enum: PipeDeleteEdges
    input: SingleInputNode
}

impl PipeDeleteEdgesNode {
    pub fn new(
        id: i64,
        info: EdgeDeleteInfo,
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) -> Self {
        Self {
            id,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            info,
            output_var: None,
            col_names: vec!["deleted".to_string()],
        }
    }

    pub fn info(&self) -> &EdgeDeleteInfo {
        &self.info
    }

    pub fn space_name(&self) -> &str {
        &self.info.space_name
    }

    pub fn edge_type(&self) -> Option<&str> {
        self.info.edge_type.as_deref()
    }

    pub fn edges(
        &self,
    ) -> &[(
        ContextualExpression,
        ContextualExpression,
        Option<ContextualExpression>,
    )] {
        &self.info.edges
    }

    pub fn condition(&self) -> Option<&ContextualExpression> {
        self.info.condition.as_ref()
    }
}
