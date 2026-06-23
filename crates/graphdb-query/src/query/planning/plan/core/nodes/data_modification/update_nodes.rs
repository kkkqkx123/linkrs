//! Update Operation Plan Nodes
//!
//! Provides plan nodes for UPDATE VERTEX and UPDATE EDGE operations.

use crate::define_plan_node;

use super::info::{EdgeUpdateInfo, UpdateTargetType, VertexUpdateInfo};

define_plan_node! {
    pub struct UpdateNode {
        info: UpdateTargetType,
    }
    enum: Update
    input: ZeroInputNode
}

impl UpdateNode {
    pub fn new(id: i64, info: UpdateTargetType) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: vec!["updated".to_string()],
        }
    }

    pub fn info(&self) -> &UpdateTargetType {
        &self.info
    }
}

define_plan_node! {
    pub struct UpdateVerticesNode {
        updates: Vec<VertexUpdateInfo>,
    }
    enum: UpdateVertices
    input: ZeroInputNode
}

impl UpdateVerticesNode {
    pub fn new(id: i64, updates: Vec<VertexUpdateInfo>) -> Self {
        Self {
            id,
            updates,
            output_var: None,
            col_names: vec!["updated".to_string()],
        }
    }

    pub fn updates(&self) -> &[VertexUpdateInfo] {
        &self.updates
    }
}

define_plan_node! {
    pub struct UpdateEdgesNode {
        updates: Vec<EdgeUpdateInfo>,
    }
    enum: UpdateEdges
    input: ZeroInputNode
}

impl UpdateEdgesNode {
    pub fn new(id: i64, updates: Vec<EdgeUpdateInfo>) -> Self {
        Self {
            id,
            updates,
            output_var: None,
            col_names: vec!["updated".to_string()],
        }
    }

    pub fn updates(&self) -> &[EdgeUpdateInfo] {
        &self.updates
    }
}
