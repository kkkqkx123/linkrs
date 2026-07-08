//! Vector Index Management Plan Nodes
//!
//! This module defines plan nodes for vector index management operations.

use crate::query::parser::ast::vector::VectorDistance;
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, ZeroInputNode};
use serde::{Deserialize, Serialize};

/// Parameters for creating a vector index node
#[derive(Debug, Clone)]
pub struct CreateVectorIndexParams {
    pub index_name: String,
    pub space_name: String,
    pub tag_name: String,
    pub field_name: String,
    pub vector_size: usize,
    pub distance: VectorDistance,
    pub hnsw_m: Option<usize>,
    pub hnsw_ef_construct: Option<usize>,
    pub if_not_exists: bool,
    pub space_id: u64,
}

impl CreateVectorIndexParams {
    pub fn new(
        index_name: String,
        space_name: String,
        tag_name: String,
        field_name: String,
        vector_size: usize,
        distance: VectorDistance,
        space_id: u64,
    ) -> Self {
        Self {
            index_name,
            space_name,
            tag_name,
            field_name,
            vector_size,
            distance,
            hnsw_m: None,
            hnsw_ef_construct: None,
            if_not_exists: false,
            space_id,
        }
    }

    pub fn with_hnsw_m(mut self, m: Option<usize>) -> Self {
        self.hnsw_m = m;
        self
    }

    pub fn with_hnsw_ef_construct(mut self, ef_construct: Option<usize>) -> Self {
        self.hnsw_ef_construct = ef_construct;
        self
    }

    pub fn with_if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }
}

/// Create vector index plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateVectorIndexNode {
    id: i64,
    pub index_name: String,
    pub space_name: String,
    pub tag_name: String,
    pub field_name: String,
    pub vector_size: usize,
    pub distance: VectorDistance,
    pub hnsw_m: Option<usize>,
    pub hnsw_ef_construct: Option<usize>,
    pub if_not_exists: bool,
    pub space_id: u64,
}

impl CreateVectorIndexNode {
    pub fn new(params: CreateVectorIndexParams) -> Self {
        Self {
            id: next_node_id(),
            index_name: params.index_name,
            space_name: params.space_name,
            tag_name: params.tag_name,
            field_name: params.field_name,
            vector_size: params.vector_size,
            distance: params.distance,
            hnsw_m: params.hnsw_m,
            hnsw_ef_construct: params.hnsw_ef_construct,
            if_not_exists: params.if_not_exists,
            space_id: params.space_id,
        }
    }
}

impl PlanNode for CreateVectorIndexNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "CreateVectorIndex"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::Management
    }

    fn output_var(&self) -> Option<&str> {
        None
    }

    fn col_names(&self) -> &[String] {
        &[]
    }

    fn set_output_var(&mut self, _var: String) {}

    fn set_col_names(&mut self, _names: Vec<String>) {}

    fn into_enum(
        self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::VectorManage(
            VectorManageNode::Create(self),
        )
    }
}

impl ZeroInputNode for CreateVectorIndexNode {}

/// Drop vector index plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropVectorIndexNode {
    id: i64,
    pub index_name: String,
    pub space_name: String,
    pub if_exists: bool,
}

impl DropVectorIndexNode {
    pub fn new(index_name: String, space_name: String, if_exists: bool) -> Self {
        Self {
            id: next_node_id(),
            index_name,
            space_name,
            if_exists,
        }
    }
}

impl PlanNode for DropVectorIndexNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "DropVectorIndex"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::Management
    }

    fn output_var(&self) -> Option<&str> {
        None
    }

    fn col_names(&self) -> &[String] {
        &[]
    }

    fn set_output_var(&mut self, _var: String) {}

    fn set_col_names(&mut self, _names: Vec<String>) {}

    fn into_enum(
        self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::VectorManage(
            VectorManageNode::Drop(self),
        )
    }
}

impl ZeroInputNode for DropVectorIndexNode {}

impl MemoryEstimatable for CreateVectorIndexNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.index_name.capacity()
            + self.space_name.capacity()
            + self.tag_name.capacity()
            + self.field_name.capacity()
    }
}

impl MemoryEstimatable for DropVectorIndexNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<Self>() + self.index_name.capacity() + self.space_name.capacity()
    }
}
