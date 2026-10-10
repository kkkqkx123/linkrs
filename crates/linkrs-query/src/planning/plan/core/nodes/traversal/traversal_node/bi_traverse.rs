use crate::define_binary_input_node;
use crate::planning::plan::core::node_id_generator::next_node_id;
use linkrs_core::types::EdgeDirection;

define_binary_input_node! {
    pub struct BiExpandNode {
        space_id: u64,
        left_direction: EdgeDirection,
        right_direction: EdgeDirection,
        edge_types: Vec<String>,
        max_hops: usize,
        meeting_point_var: Option<String>,
        dst_tag: Option<String>,
    }
    enum: BiExpand
    input: BinaryInputNode
}

impl BiExpandNode {
    pub fn new(
        left_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        space_id: u64,
        left_direction: EdgeDirection,
        right_direction: EdgeDirection,
        edge_types: Vec<String>,
        max_hops: usize,
    ) -> Self {
        Self {
            id: next_node_id(),
            left: Box::new(left_input),
            right: Box::new(right_input),
            space_id,
            left_direction,
            right_direction,
            edge_types,
            max_hops,
            meeting_point_var: None,
            dst_tag: None,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn dst_tag(&self) -> Option<&str> {
        self.dst_tag.as_deref()
    }

    pub fn set_dst_tag(&mut self, tag: String) {
        self.dst_tag = Some(tag);
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn left_direction(&self) -> EdgeDirection {
        self.left_direction
    }

    pub fn right_direction(&self) -> EdgeDirection {
        self.right_direction
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn max_hops(&self) -> usize {
        self.max_hops
    }

    pub fn meeting_point_var(&self) -> Option<&String> {
        self.meeting_point_var.as_ref()
    }

    pub fn set_meeting_point_var(&mut self, var: String) {
        self.meeting_point_var = Some(var);
    }
}

define_binary_input_node! {
    pub struct BiTraverseNode {
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
        dst_tag: Option<String>,
    }
    enum: BiTraverse
    input: BinaryInputNode
}

/// Parameters for creating BiTraverseNode
pub struct BiTraverseNodeParams {
    pub left_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    pub right_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    pub space_id: u64,
    pub left_src_var: String,
    pub right_src_var: String,
    pub edge_types: Vec<String>,
    pub left_direction: EdgeDirection,
    pub right_direction: EdgeDirection,
    pub min_hops: usize,
    pub max_hops: usize,
    pub path_var: String,
}

impl BiTraverseNode {
    pub fn new(params: BiTraverseNodeParams) -> Self {
        Self {
            id: next_node_id(),
            left: Box::new(params.left_input),
            right: Box::new(params.right_input),
            space_id: params.space_id,
            left_src_var: params.left_src_var,
            right_src_var: params.right_src_var,
            edge_types: params.edge_types,
            left_direction: params.left_direction,
            right_direction: params.right_direction,
            min_hops: params.min_hops,
            max_hops: params.max_hops,
            path_var: params.path_var.clone(),
            edge_alias: None,
            vertex_alias: None,
            dst_tag: None,
            output_var: Some(params.path_var),
            col_names: vec![],
            column_types: vec![],
        }
    }

    pub fn dst_tag(&self) -> Option<&str> {
        self.dst_tag.as_deref()
    }

    pub fn set_dst_tag(&mut self, tag: String) {
        self.dst_tag = Some(tag);
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn left_src_var(&self) -> &str {
        &self.left_src_var
    }

    pub fn right_src_var(&self) -> &str {
        &self.right_src_var
    }

    pub fn edge_types(&self) -> &[String] {
        &self.edge_types
    }

    pub fn left_direction(&self) -> EdgeDirection {
        self.left_direction
    }

    pub fn right_direction(&self) -> EdgeDirection {
        self.right_direction
    }

    pub fn min_hops(&self) -> usize {
        self.min_hops
    }

    pub fn max_hops(&self) -> usize {
        self.max_hops
    }

    pub fn path_var(&self) -> &str {
        &self.path_var
    }

    pub fn edge_alias(&self) -> Option<&String> {
        self.edge_alias.as_ref()
    }

    pub fn set_edge_alias(&mut self, alias: String) {
        self.edge_alias = Some(alias);
    }

    pub fn vertex_alias(&self) -> Option<&String> {
        self.vertex_alias.as_ref()
    }

    pub fn set_vertex_alias(&mut self, alias: String) {
        self.vertex_alias = Some(alias);
    }
}
