//! Logical access nodes: Start, GetVertices, GetEdges, GetNeighbors, ScanVertices, ScanEdges.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::define_logical_plan_node;
use crate::query::planning::plan::core::common::{EdgeProp, TagProp};

define_logical_plan_node! {
    pub struct LogicalStartNode {}
    enum: Start
    input: ZeroInputNode
}

impl LogicalStartNode {
    pub fn new() -> Self {
        Self {
            id: -1,
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        }
    }
}

define_logical_plan_node! {
    pub struct LogicalGetVerticesNode {
        space_id: u64,
        space_name: String,
        src_ref: ContextualExpression,
        src_vids: String,
        tag_props: Vec<TagProp>,
        expression: Option<ContextualExpression>,
        dedup: bool,
        limit: Option<i64>,
    }
    enum: GetVertices
    input: MultipleInputNode
}

define_logical_plan_node! {
    pub struct LogicalGetEdgesNode {
        space_id: u64,
        edge_ref: ContextualExpression,
        src: String,
        edge_type: String,
        rank: String,
        dst: String,
        edge_props: Vec<EdgeProp>,
        expression: Option<ContextualExpression>,
        dedup: bool,
        limit: Option<i64>,
    }
    enum: GetEdges
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalGetNeighborsNode {
        space_id: u64,
        src_vids: String,
        edge_types: Vec<String>,
        direction: String,
        edge_props: Vec<EdgeProp>,
        tag_props: Vec<TagProp>,
        expression: Option<ContextualExpression>,
        dedup: bool,
        limit: Option<i64>,
    }
    enum: GetNeighbors
    input: MultipleInputNode
}

define_logical_plan_node! {
    pub struct LogicalScanVerticesNode {
        space_id: u64,
        space_name: String,
        tag: Option<String>,
        expression: Option<ContextualExpression>,
        limit: Option<i64>,
        projected_properties: Vec<String>,
    }
    enum: ScanVertices
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalScanEdgesNode {
        space_id: u64,
        edge_type: Option<String>,
        expression: Option<ContextualExpression>,
        limit: Option<i64>,
        projected_properties: Vec<String>,
    }
    enum: ScanEdges
    input: ZeroInputNode
}
