//! Logical operation nodes: Project, Filter, Sort, Limit, TopN, Sample, Dedup, Aggregate, Window.

use crate::define_logical_plan_node_with_deps;
use crate::planning::plan::core::nodes::graph_operations::window_node::WindowFunctionSpec;
use crate::planning::plan::core::nodes::operation::sort_node::SortItem;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::operators::AggregateFunction;
use graphdb_core::Expression;
use graphdb_core::YieldColumn;

define_logical_plan_node_with_deps! {
    pub struct LogicalProjectNode {
        columns: Vec<YieldColumn>,
        subqueries: Vec<crate::planning::statements::clauses::exists_planner::PlannedSubquery>,
        has_folded_expressions: bool,
    }
    enum: Project
    input: SingleInputNode
}

impl LogicalProjectNode {
    /// Attach expression-level subqueries to this logical projection.
    pub fn with_subqueries(
        mut self,
        subqueries: Vec<crate::planning::statements::clauses::exists_planner::PlannedSubquery>,
    ) -> Self {
        self.subqueries = subqueries;
        self
    }

    /// Expression-level subqueries compiled for this projection.
    pub fn subqueries(
        &self,
    ) -> &[crate::planning::statements::clauses::exists_planner::PlannedSubquery] {
        &self.subqueries
    }
}

define_logical_plan_node_with_deps! {
    pub struct LogicalFilterNode {
        condition: ContextualExpression,
    }
    enum: Filter
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalSortNode {
        sort_items: Vec<SortItem>,
        limit: Option<i64>,
    }
    enum: Sort
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalLimitNode {
        offset: i64,
        count: i64,
    }
    enum: Limit
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalTopNNode {
        sort_items: Vec<SortItem>,
        limit: i64,
    }
    enum: TopN
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalSampleNode {
        count: i64,
    }
    enum: Sample
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalDedupNode {}
    enum: Dedup
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalAggregateNode {
        group_key_exprs: Vec<ContextualExpression>,
        aggregation_functions: Vec<AggregateFunction>,
        aggregation_args: Vec<Vec<Expression>>,
        aggregation_distinct: Vec<bool>,
        aggregation_filters: Vec<Option<Expression>>,
        grouping_sets: Vec<Vec<String>>,
    }
    enum: Aggregate
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalWindowNode {
        window_functions: Vec<WindowFunctionSpec>,
    }
    enum: Window
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalSkipNode {
        offset: i64,
    }
    enum: Skip
    input: SingleInputNode
}
