//! Full-Text Index Management Plan Nodes
//!
//! This module defines plan nodes for full-text index management operations.

use crate::core::types::FulltextEngineType;
use crate::query::parser::ast::fulltext::{AlterIndexAction, IndexFieldDef, IndexOptions};
use crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, ZeroInputNode};
use serde::{Deserialize, Serialize};

/// CREATE FULLTEXT INDEX plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateFulltextIndexNode {
    pub index_name: String,
    pub schema_name: String,
    pub fields: Vec<IndexFieldDef>,
    pub engine_type: FulltextEngineType,
    pub options: IndexOptions,
    pub if_not_exists: bool,
    pub space_id: u64,
}

impl CreateFulltextIndexNode {
    pub fn new(
        index_name: String,
        schema_name: String,
        fields: Vec<IndexFieldDef>,
        engine_type: FulltextEngineType,
        options: IndexOptions,
        if_not_exists: bool,
        space_id: u64,
    ) -> Self {
        Self {
            index_name,
            schema_name,
            fields,
            engine_type,
            options,
            if_not_exists,
            space_id,
        }
    }
}

impl PlanNode for CreateFulltextIndexNode {
    fn id(&self) -> i64 {
        0
    }

    fn name(&self) -> &'static str {
        "CreateFulltextIndex"
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
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::FulltextManage(FulltextManageNode::Create(self))
    }
}

impl ZeroInputNode for CreateFulltextIndexNode {}

/// DROP FULLTEXT INDEX plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DropFulltextIndexNode {
    pub index_name: String,
    pub if_exists: bool,
}

impl DropFulltextIndexNode {
    pub fn new(index_name: String, if_exists: bool) -> Self {
        Self {
            index_name,
            if_exists,
        }
    }
}

impl PlanNode for DropFulltextIndexNode {
    fn id(&self) -> i64 {
        0
    }

    fn name(&self) -> &'static str {
        "DropFulltextIndex"
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
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::FulltextManage(FulltextManageNode::Drop(self))
    }
}

impl ZeroInputNode for DropFulltextIndexNode {}

/// ALTER FULLTEXT INDEX plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlterFulltextIndexNode {
    pub index_name: String,
    pub actions: Vec<AlterIndexAction>,
}

impl AlterFulltextIndexNode {
    pub fn new(index_name: String, actions: Vec<AlterIndexAction>) -> Self {
        Self {
            index_name,
            actions,
        }
    }
}

impl PlanNode for AlterFulltextIndexNode {
    fn id(&self) -> i64 {
        0
    }

    fn name(&self) -> &'static str {
        "AlterFulltextIndex"
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
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::FulltextManage(FulltextManageNode::Alter(self))
    }
}

impl ZeroInputNode for AlterFulltextIndexNode {}

/// SHOW FULLTEXT INDEX plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShowFulltextIndexNode {
    pub pattern: Option<String>,
    pub from_schema: Option<String>,
}

impl ShowFulltextIndexNode {
    pub fn new(pattern: Option<String>, from_schema: Option<String>) -> Self {
        Self {
            pattern,
            from_schema,
        }
    }
}

impl PlanNode for ShowFulltextIndexNode {
    fn id(&self) -> i64 {
        0
    }

    fn name(&self) -> &'static str {
        "ShowFulltextIndex"
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
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::FulltextManage(FulltextManageNode::Show(self))
    }
}

impl ZeroInputNode for ShowFulltextIndexNode {}

/// DESCRIBE FULLTEXT INDEX plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DescribeFulltextIndexNode {
    pub index_name: String,
}

impl DescribeFulltextIndexNode {
    pub fn new(index_name: String) -> Self {
        Self { index_name }
    }
}

impl PlanNode for DescribeFulltextIndexNode {
    fn id(&self) -> i64 {
        0
    }

    fn name(&self) -> &'static str {
        "DescribeFulltextIndex"
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
        use crate::query::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::FulltextManage(FulltextManageNode::Describe(self))
    }
}

impl ZeroInputNode for DescribeFulltextIndexNode {}

impl MemoryEstimatable for CreateFulltextIndexNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<CreateFulltextIndexNode>();
        let index_name_size = std::mem::size_of::<String>() + self.index_name.capacity();
        let schema_name_size = std::mem::size_of::<String>() + self.schema_name.capacity();
        let fields_size = std::mem::size_of::<Vec<IndexFieldDef>>()
            + self
                .fields
                .iter()
                .map(|f| std::mem::size_of::<IndexFieldDef>() + f.field_name.capacity())
                .sum::<usize>();
        let options_size = std::mem::size_of::<IndexOptions>();
        base + index_name_size + schema_name_size + fields_size + options_size
    }
}

impl MemoryEstimatable for DropFulltextIndexNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<DropFulltextIndexNode>();
        let index_name_size = std::mem::size_of::<String>() + self.index_name.capacity();
        base + index_name_size
    }
}

impl MemoryEstimatable for AlterFulltextIndexNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<AlterFulltextIndexNode>();
        let index_name_size = std::mem::size_of::<String>() + self.index_name.capacity();
        let actions_size = std::mem::size_of::<Vec<AlterIndexAction>>();
        base + index_name_size + actions_size
    }
}

impl MemoryEstimatable for ShowFulltextIndexNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<ShowFulltextIndexNode>();
        let pattern_size = self
            .pattern
            .as_ref()
            .map(|s| std::mem::size_of::<String>() + s.capacity())
            .unwrap_or(0);
        let from_schema_size = self
            .from_schema
            .as_ref()
            .map(|s| std::mem::size_of::<String>() + s.capacity())
            .unwrap_or(0);
        base + pattern_size + from_schema_size
    }
}

impl MemoryEstimatable for DescribeFulltextIndexNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<DescribeFulltextIndexNode>();
        let index_name_size = std::mem::size_of::<String>() + self.index_name.capacity();
        base + index_name_size
    }
}
