//! Logical algorithm nodes: MultiShortestPath, BFSShortest, AllPaths, ShortestPath.

use crate::define_logical_binary_input_node;
use graphdb_core::types::EdgeDirection;
use graphdb_core::types::VertexId;
use graphdb_core::Value;

define_logical_binary_input_node! {
    pub struct LogicalMultiShortestPathNode {
        steps: usize,
        left_vid_var: String,
        right_vid_var: String,
        termination_var: String,
        single_shortest: bool,
        edge_types: Vec<String>,
        direction: EdgeDirection,
        target_vertex_ids: Vec<Value>,
    }
    enum: MultiShortestPath
    input: BinaryInputNode
}

define_logical_binary_input_node! {
    pub struct LogicalBFSShortestNode {
        space_id: u64,
        steps: usize,
        edge_types: Vec<String>,
        with_cycle: bool,
        with_loop: bool,
        reverse: bool,
    }
    enum: BFSShortest
    input: BinaryInputNode
}

define_logical_binary_input_node! {
    pub struct LogicalAllPathsNode {
        space_id: u64,
        steps: usize,
        edge_types: Vec<String>,
        min_hop: usize,
        max_hop: usize,
        acyclic: bool,
        direction: EdgeDirection,
        has_step_limit: bool,
        limit: i64,
        offset: i64,
        filter: Option<graphdb_core::types::expr::contextual::ContextualExpression>,
        start_vertex_ids: Vec<VertexId>,
        end_vertex_ids: Vec<VertexId>,
    }
    enum: AllPaths
    input: BinaryInputNode
}

define_logical_binary_input_node! {
    pub struct LogicalShortestPathNode {
        space_id: u64,
        edge_types: Vec<String>,
        max_step: usize,
        weight_expression: Option<String>,
        heuristic_expression: Option<String>,
        no_reverse: bool,
        start_vertex_ids: Vec<Value>,
        end_vertex_ids: Vec<Value>,
    }
    enum: ShortestPath
    input: BinaryInputNode
}
