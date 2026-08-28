//! Logical traversal nodes: Expand, ExpandAll, Traverse, AppendVertices, BiExpand, BiTraverse.

use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::EdgeDirection;
use graphdb_core::Value;
use crate::define_logical_binary_input_node;
use crate::define_logical_plan_node;
use crate::define_logical_plan_node_with_deps;
use crate::planning::plan::core::common::{EdgeProp, TagProp};

define_logical_plan_node! {
    pub struct LogicalExpandNode {
        space_id: u64,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        step_limit: Option<u32>,
        filter: Option<ContextualExpression>,
    }
    enum: Expand
    input: MultipleInputNode
}

define_logical_plan_node! {
    pub struct LogicalExpandAllNode {
        space_id: u64,
        edge_types: Vec<String>,
        direction: String,
        any_edge_type: bool,
        step_limit: Option<u32>,
        step_limits: Option<Vec<u32>>,
        join_input: bool,
        sample: bool,
        edge_props: Vec<EdgeProp>,
        vertex_props: Vec<TagProp>,
        filter: Option<ContextualExpression>,
        src_vids: Vec<Value>,
        include_empty_paths: bool,
        input_var: Option<String>,
    }
    enum: ExpandAll
    input: MultipleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalTraverseNode {
        space_id: u64,
        start_vids: String,
        end_vids: Option<String>,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        min_steps: u32,
        max_steps: u32,
        edge_alias: Option<String>,
        vertex_alias: Option<String>,
        e_filter: Option<ContextualExpression>,
        v_filter: Option<ContextualExpression>,
        first_step_filter: Option<ContextualExpression>,
    }
    enum: Traverse
    input: SingleInputNode
}

define_logical_plan_node! {
    pub struct LogicalAppendVerticesNode {
        space_id: u64,
        vertex_tag: String,
        vertex_props: Vec<TagProp>,
        filter: Option<ContextualExpression>,
        input_var: Option<String>,
        src_expression: Option<ContextualExpression>,
        dedup: bool,
        need_fetch_prop: bool,
        vids: Vec<String>,
        tag_ids: Vec<i32>,
        v_filter: Option<ContextualExpression>,
        node_alias: Option<String>,
    }
    enum: AppendVertices
    input: MultipleInputNode
}

define_logical_binary_input_node! {
    pub struct LogicalBiExpandNode {
        space_id: u64,
        left_direction: EdgeDirection,
        right_direction: EdgeDirection,
        edge_types: Vec<String>,
        max_hops: usize,
        meeting_point_var: Option<String>,
    }
    enum: BiExpand
    input: BinaryInputNode
}

define_logical_binary_input_node! {
    pub struct LogicalBiTraverseNode {
        space_id: u64,
        left_src_var: String,
        right_src_var: String,
        edge_types: Vec<String>,
        left_direction: EdgeDirection,
        right_direction: EdgeDirection,
        min_hops: usize,
        max_hops: usize,
        path_var: String,
        edge_alias: Option<String>,
        vertex_alias: Option<String>,
    }
    enum: BiTraverse
    input: BinaryInputNode
}
