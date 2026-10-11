//! Physical-to-logical conversion: operation node families.

use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;

use super::{convert_plan, ConversionError};

use crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::DedupNode;
use crate::planning::plan::core::nodes::graph_operations::window_node::WindowNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::planning::plan::core::nodes::operation::sample_node::SampleNode;
use crate::planning::plan::core::nodes::operation::sort_node::{LimitNode, SortNode, TopNNode};

use crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

pub(super) fn convert_project(n: &ProjectNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Project(
            crate::planning::plan::logical::logical_nodes::operation::LogicalProjectNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                columns: n.columns().to_vec(),
                subqueries: n.subqueries().to_vec(),
                has_folded_expressions: n.has_folded_expressions(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_filter(n: &FilterNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Filter(
            crate::planning::plan::logical::logical_nodes::operation::LogicalFilterNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                condition: n.condition().clone(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_sort(n: &SortNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Sort(
            crate::planning::plan::logical::logical_nodes::operation::LogicalSortNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                sort_items: n.sort_items().to_vec(),
                limit: n.limit(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_limit(n: &LimitNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        if n.count() == i64::MAX {
            return Ok(LogicalNodeEnum::Skip(
                crate::planning::plan::logical::logical_nodes::operation::LogicalSkipNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input)),
                    offset: n.offset(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ));
        }
        Ok(LogicalNodeEnum::Limit(
            crate::planning::plan::logical::logical_nodes::operation::LogicalLimitNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                offset: n.offset(),
                count: n.count(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_top_n(n: &TopNNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::TopN(
            crate::planning::plan::logical::logical_nodes::operation::LogicalTopNNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                sort_items: n.sort_items().to_vec(),
                limit: n.limit(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_sample(n: &SampleNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Sample(
            crate::planning::plan::logical::logical_nodes::operation::LogicalSampleNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                count: n.count(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_dedup(n: &DedupNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Dedup(
            crate::planning::plan::logical::logical_nodes::operation::LogicalDedupNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}

pub(super) fn convert_aggregate(n: &AggregateNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        if let Some(exprs) = n.group_key_exprs() {
            Ok(LogicalNodeEnum::Aggregate(
                crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode {
                    id: n.id(),
                    input: Some(Box::new(logical_input)),
                    group_key_exprs: exprs.to_vec(),
                    aggregation_functions: n.aggregation_functions().to_vec(),
                    aggregation_args: n.aggregation_args().to_vec(),
                    aggregation_distinct: n.aggregation_distinct().to_vec(),
                    aggregation_filters: n.aggregation_filters().to_vec(),
                    grouping_sets: n.grouping_sets().to_vec(),
                    output_var: n.output_var().map(|s| s.to_string()),
                    col_names: n.col_names().to_vec(),
                    column_types: n.column_types().to_vec(),
                },
            ))
        } else {
            Err(ConversionError::NotYetImplemented(
                    "Aggregate: group_keys string conversion synthesizes ExpressionId; use binder LogicalAggregateNode with group_key_exprs or upgrade physical node via physical_planner group_key_exprs pass-through".to_string(),
                ))
        }
    }
}

pub(super) fn convert_window(n: &WindowNode) -> Result<LogicalNodeEnum, ConversionError> {
    {
        let logical_input = convert_plan(n.input())?;
        Ok(LogicalNodeEnum::Window(
            crate::planning::plan::logical::logical_nodes::operation::LogicalWindowNode {
                id: n.id(),
                input: Some(Box::new(logical_input)),
                window_functions: n.window_functions().to_vec(),
                output_var: n.output_var().map(|s| s.to_string()),
                col_names: n.col_names().to_vec(),
                column_types: n.column_types().to_vec(),
            },
        ))
    }
}
