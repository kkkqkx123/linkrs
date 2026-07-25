//! Logical operation nodes: Project, Filter, Sort, Limit, TopN, Sample, Dedup, Aggregate, Window.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::operators::AggregateFunction;
use crate::core::Expression;
use crate::core::YieldColumn;
use crate::define_logical_plan_node_with_deps;
use crate::query::planning::plan::core::nodes::graph_operations::window_node::WindowFunctionSpec;
use crate::query::planning::plan::core::nodes::operation::sort_node::SortItem;

define_logical_plan_node_with_deps! {
    pub struct LogicalProjectNode {
        columns: Vec<YieldColumn>,
    }
    enum: Project
    input: SingleInputNode
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
        group_keys: Vec<String>,
        aggregation_functions: Vec<AggregateFunction>,
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
