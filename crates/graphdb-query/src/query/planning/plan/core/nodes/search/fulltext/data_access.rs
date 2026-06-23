//! Full-Text Search Data Access Plan Nodes
//!
//! This module defines plan nodes for full-text search data access operations.

use crate::query::parser::ast::fulltext::{
    FulltextMatchCondition, FulltextQueryExpr, FulltextYieldClause, OrderClause, WhereClause,
};
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, ZeroInputNode};
use serde::{Deserialize, Serialize};

/// Full-text search plan node (SEARCH statement)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextSearchNode {
    id: i64,
    pub index_name: String,
    pub query: FulltextQueryExpr,
    pub yield_clause: Option<FulltextYieldClause>,
    pub where_clause: Option<WhereClause>,
    pub order_clause: Option<OrderClause>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    /// Pre-resolved space_id from metadata context
    pub space_id: u64,
    /// Pre-resolved tag_name from index metadata
    pub tag_name: String,
    /// Pre-resolved field_name from index metadata
    pub field_name: String,
}

impl FulltextSearchNode {
    pub fn new(
        index_name: String,
        query: FulltextQueryExpr,
        yield_clause: Option<FulltextYieldClause>,
        where_clause: Option<WhereClause>,
        order_clause: Option<OrderClause>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Self {
        Self {
            id: next_node_id(),
            index_name,
            query,
            yield_clause,
            where_clause,
            order_clause,
            limit,
            offset,
            space_id: 0,
            tag_name: String::new(),
            field_name: String::new(),
        }
    }

    pub fn with_metadata(mut self, space_id: u64, tag_name: String, field_name: String) -> Self {
        self.space_id = space_id;
        self.tag_name = tag_name;
        self.field_name = field_name;
        self
    }

    pub fn id(&self) -> i64 {
        self.id
    }
}

impl PlanNode for FulltextSearchNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "FulltextSearch"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::DataAccess
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
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::FulltextSearch(self)
    }
}

impl ZeroInputNode for FulltextSearchNode {}

/// Full-text lookup plan node (LOOKUP FULLTEXT statement)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FulltextLookupNode {
    id: i64,
    pub schema_name: String,
    pub index_name: String,
    pub query: String,
    pub yield_clause: Option<FulltextYieldClause>,
    pub limit: Option<usize>,
    /// Pre-resolved space_id from metadata context
    pub space_id: u64,
    /// Pre-resolved tag_name from index metadata
    pub tag_name: String,
    /// Pre-resolved field_name from index metadata
    pub field_name: String,
}

impl FulltextLookupNode {
    pub fn new(
        schema_name: String,
        index_name: String,
        query: String,
        yield_clause: Option<FulltextYieldClause>,
        limit: Option<usize>,
    ) -> Self {
        Self {
            id: next_node_id(),
            schema_name,
            index_name,
            query,
            yield_clause,
            limit,
            space_id: 0,
            tag_name: String::new(),
            field_name: String::new(),
        }
    }

    pub fn with_metadata(mut self, space_id: u64, tag_name: String, field_name: String) -> Self {
        self.space_id = space_id;
        self.tag_name = tag_name;
        self.field_name = field_name;
        self
    }

    pub fn id(&self) -> i64 {
        self.id
    }
}

impl PlanNode for FulltextLookupNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "FulltextLookup"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::DataAccess
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
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::FulltextLookup(self)
    }
}

impl ZeroInputNode for FulltextLookupNode {}

/// Match with full-text plan node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchFulltextNode {
    pub pattern: String,
    pub fulltext_condition: FulltextMatchCondition,
    pub yield_clause: Option<FulltextYieldClause>,
    /// Pre-resolved space_id from metadata context
    pub space_id: u64,
    /// Pre-resolved tag_name from index metadata
    pub tag_name: String,
    /// Pre-resolved field_name from index metadata
    pub field_name: String,
}

impl MatchFulltextNode {
    pub fn new(
        pattern: String,
        fulltext_condition: FulltextMatchCondition,
        yield_clause: Option<FulltextYieldClause>,
    ) -> Self {
        Self {
            pattern,
            fulltext_condition,
            yield_clause,
            space_id: 0,
            tag_name: String::new(),
            field_name: String::new(),
        }
    }

    pub fn with_metadata(mut self, space_id: u64, tag_name: String, field_name: String) -> Self {
        self.space_id = space_id;
        self.tag_name = tag_name;
        self.field_name = field_name;
        self
    }
}

impl PlanNode for MatchFulltextNode {
    fn id(&self) -> i64 {
        0
    }

    fn name(&self) -> &'static str {
        "MatchFulltext"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::DataAccess
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
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::MatchFulltext(
            self,
        )
    }
}

impl ZeroInputNode for MatchFulltextNode {}

impl MemoryEstimatable for FulltextSearchNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<FulltextSearchNode>();
        let index_name_size = std::mem::size_of::<String>() + self.index_name.capacity();
        let query_size = std::mem::size_of::<FulltextQueryExpr>();
        let yield_size = self
            .yield_clause
            .as_ref()
            .map(|_| std::mem::size_of::<FulltextYieldClause>())
            .unwrap_or(0);
        let where_size = self
            .where_clause
            .as_ref()
            .map(|_| std::mem::size_of::<WhereClause>())
            .unwrap_or(0);
        let order_size = self
            .order_clause
            .as_ref()
            .map(|_| std::mem::size_of::<OrderClause>())
            .unwrap_or(0);
        let limit_size = self
            .limit
            .map(|_| std::mem::size_of::<usize>())
            .unwrap_or(0);
        let offset_size = self
            .offset
            .map(|_| std::mem::size_of::<usize>())
            .unwrap_or(0);
        base + index_name_size
            + query_size
            + yield_size
            + where_size
            + order_size
            + limit_size
            + offset_size
    }
}

impl MemoryEstimatable for FulltextLookupNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<FulltextLookupNode>();
        let schema_name_size = std::mem::size_of::<String>() + self.schema_name.capacity();
        let index_name_size = std::mem::size_of::<String>() + self.index_name.capacity();
        let query_size = std::mem::size_of::<String>() + self.query.capacity();
        let yield_size = self
            .yield_clause
            .as_ref()
            .map(|_| std::mem::size_of::<FulltextYieldClause>())
            .unwrap_or(0);
        let limit_size = self
            .limit
            .map(|_| std::mem::size_of::<usize>())
            .unwrap_or(0);
        base + schema_name_size + index_name_size + query_size + yield_size + limit_size
    }
}

impl MemoryEstimatable for MatchFulltextNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<MatchFulltextNode>();
        let pattern_size = std::mem::size_of::<String>() + self.pattern.capacity();
        let condition_size = std::mem::size_of::<FulltextMatchCondition>();
        let yield_size = self
            .yield_clause
            .as_ref()
            .map(|_| std::mem::size_of::<FulltextYieldClause>())
            .unwrap_or(0);
        base + pattern_size + condition_size + yield_size
    }
}
