//! Plan nodes related to pathfinding algorithms
//! Plan nodes related to algorithms such as the shortest path, all paths, etc.
//!
//! The selection of the algorithm was completed during the Planner phase; this module only contains the planning nodes related to the specific algorithm.

use crate::core::types::{ContextualExpression, VertexId};
use crate::core::Value;
use crate::define_binary_input_node;
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

define_binary_input_node! {
    /// Multi-source shortest path planning nodes
    pub struct MultiShortestPathNode {
        steps: usize,
        left_vid_var: String,
        right_vid_var: String,
        termination_var: String,
        single_shortest: bool,
    }
    enum: MultiShortestPath
    input: BinaryInputNode
}

impl MultiShortestPathNode {
    pub fn new(left: PlanNodeEnum, right: PlanNodeEnum, steps: usize) -> Self {
        let left_box = Box::new(left);
        let right_box = Box::new(right);
        Self {
            id: next_node_id(),
            left: left_box.clone(),
            right: right_box.clone(),
            deps: vec![*left_box, *right_box],
            steps,
            left_vid_var: String::new(),
            right_vid_var: String::new(),
            termination_var: String::new(),
            single_shortest: false,
            output_var: None,
            col_names: vec!["path".to_string()],
        }
    }

    pub fn steps(&self) -> usize {
        self.steps
    }

    pub fn left_vid_var(&self) -> &str {
        &self.left_vid_var
    }

    pub fn right_vid_var(&self) -> &str {
        &self.right_vid_var
    }

    pub fn termination_var(&self) -> &str {
        &self.termination_var
    }

    pub fn single_shortest(&self) -> bool {
        self.single_shortest
    }

    pub fn set_left_vid_var(&mut self, var: &str) {
        self.left_vid_var = var.to_string();
    }

    pub fn set_right_vid_var(&mut self, var: &str) {
        self.right_vid_var = var.to_string();
    }

    pub fn accept<V>(&self, visitor: &mut V) -> V::Result
    where
        V: crate::query::planning::plan::core::nodes::base::plan_node_visitor::PlanNodeVisitor,
    {
        visitor.visit_multi_shortest_path(self)
    }
}

define_binary_input_node! {
    /// BFS (Broad-Search First) algorithm for planning the shortest path: the nodes involved in this process
    ///
    /// Use a bidirectional BFS algorithm to find the shortest path.
    /// The selection of the algorithm was completed during the Planner phase. This node is specifically designed for bidirectional BFS (Breadth-First Search).
    pub struct BFSShortestNode {
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

impl BFSShortestNode {
    pub fn new(
        left: PlanNodeEnum,
        right: PlanNodeEnum,
        space_id: u64,
        steps: usize,
        edge_types: Vec<String>,
        with_cycle: bool,
    ) -> Self {
        let left_box = Box::new(left);
        let right_box = Box::new(right);
        Self {
            id: next_node_id(),
            left: left_box.clone(),
            right: right_box.clone(),
            deps: vec![*left_box, *right_box],
            space_id,
            steps,
            edge_types,
            with_cycle,
            with_loop: false,
            reverse: false,
            output_var: None,
            col_names: vec!["path".to_string()],
        }
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn set_loop(&mut self, with_loop: bool) {
        self.with_loop = with_loop;
    }

    pub fn set_reverse(&mut self, reverse: bool) {
        self.reverse = reverse;
    }

    pub fn steps(&self) -> usize {
        self.steps
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn with_cycle(&self) -> bool {
        self.with_cycle
    }

    pub fn with_loop(&self) -> bool {
        self.with_loop
    }

    pub fn reverse(&self) -> bool {
        self.reverse
    }

    pub fn accept<V>(&self, visitor: &mut V) -> V::Result
    where
        V: crate::query::planning::plan::core::nodes::base::plan_node_visitor::PlanNodeVisitor,
    {
        visitor.visit_bfs_shortest(self)
    }
}

define_binary_input_node! {
    /// All path planning nodes
    pub struct AllPathsNode {
        space_id: u64,
        steps: usize,
        edge_types: Vec<String>,
        min_hop: usize,
        max_hop: usize,
        acyclic: bool,
        has_step_limit: bool,
        limit: i64,
        offset: i64,
        filter: Option<ContextualExpression>,
        start_vertex_ids: Vec<VertexId>,
        end_vertex_ids: Vec<VertexId>,
    }
    enum: AllPaths
    input: BinaryInputNode
}

impl AllPathsNode {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        left: PlanNodeEnum,
        right: PlanNodeEnum,
        space_id: u64,
        steps: usize,
        edge_types: Vec<String>,
        min_hop: usize,
        max_hop: usize,
        acyclic: bool,
    ) -> Self {
        let left_box = Box::new(left);
        let right_box = Box::new(right);
        Self {
            id: next_node_id(),
            left: left_box.clone(),
            right: right_box.clone(),
            deps: vec![*left_box, *right_box],
            space_id,
            steps,
            edge_types,
            min_hop,
            max_hop,
            acyclic,
            has_step_limit: true,
            limit: -1,
            offset: 0,
            filter: None,
            start_vertex_ids: Vec::new(),
            end_vertex_ids: Vec::new(),
            output_var: None,
            col_names: vec!["path".to_string()],
        }
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn start_vertex_ids(&self) -> &[VertexId] {
        &self.start_vertex_ids
    }

    pub fn end_vertex_ids(&self) -> &[VertexId] {
        &self.end_vertex_ids
    }

    pub fn set_start_vertex_ids(&mut self, ids: Vec<VertexId>) {
        self.start_vertex_ids = ids;
    }

    pub fn set_end_vertex_ids(&mut self, ids: Vec<VertexId>) {
        self.end_vertex_ids = ids;
    }

    pub fn min_hop(&self) -> usize {
        self.min_hop
    }

    pub fn max_hop(&self) -> usize {
        self.max_hop
    }

    pub fn is_acyclic(&self) -> bool {
        self.acyclic
    }

    pub fn limit(&self) -> i64 {
        self.limit
    }

    pub fn offset(&self) -> i64 {
        self.offset
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = limit;
    }

    pub fn set_offset(&mut self, offset: i64) {
        self.offset = offset;
    }

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.filter.as_ref()
    }

    pub fn set_filter(&mut self, filter: ContextualExpression) {
        self.filter = Some(filter);
    }

    pub fn steps(&self) -> usize {
        self.steps
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn accept<V>(&self, visitor: &mut V) -> V::Result
    where
        V: crate::query::planning::plan::core::nodes::base::plan_node_visitor::PlanNodeVisitor,
    {
        visitor.visit_all_paths(self)
    }
}

define_binary_input_node! {
    /// Shortest Path Planning Node
    pub struct ShortestPathNode {
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

impl ShortestPathNode {
    pub fn new(
        left: PlanNodeEnum,
        right: PlanNodeEnum,
        space_id: u64,
        edge_types: Vec<String>,
        max_step: usize,
    ) -> Self {
        let left_box = Box::new(left);
        let right_box = Box::new(right);
        Self {
            id: next_node_id(),
            left: left_box.clone(),
            right: right_box.clone(),
            deps: vec![*left_box, *right_box],
            space_id,
            edge_types,
            max_step,
            weight_expression: None,
            heuristic_expression: None,
            no_reverse: false,
            start_vertex_ids: Vec::new(),
            end_vertex_ids: Vec::new(),
            output_var: None,
            col_names: vec!["path".to_string()],
        }
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn max_step(&self) -> usize {
        self.max_step
    }

    pub fn start_vertex_ids(&self) -> &[Value] {
        &self.start_vertex_ids
    }

    pub fn end_vertex_ids(&self) -> &[Value] {
        &self.end_vertex_ids
    }

    pub fn set_start_vertex_ids(&mut self, ids: Vec<Value>) {
        self.start_vertex_ids = ids;
    }

    pub fn set_end_vertex_ids(&mut self, ids: Vec<Value>) {
        self.end_vertex_ids = ids;
    }

    pub fn set_weight_expression(&mut self, expression: String) {
        self.weight_expression = Some(expression);
    }

    pub fn weight_expression(&self) -> &Option<String> {
        &self.weight_expression
    }

    pub fn set_heuristic_expression(&mut self, expression: String) {
        self.heuristic_expression = Some(expression);
    }

    pub fn heuristic_expression(&self) -> &Option<String> {
        &self.heuristic_expression
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn no_reverse(&self) -> bool {
        self.no_reverse
    }

    pub fn set_no_reverse(&mut self, no_reverse: bool) {
        self.no_reverse = no_reverse;
    }

    pub fn accept<V>(&self, visitor: &mut V) -> V::Result
    where
        V: crate::query::planning::plan::core::nodes::base::plan_node_visitor::PlanNodeVisitor,
    {
        visitor.visit_shortest_path(self)
    }
}
