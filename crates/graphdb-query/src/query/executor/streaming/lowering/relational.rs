//! Lower relational plan nodes (Filter, Project, Limit, Sort, Aggregate, etc.)
//! into PhysicalNode trees.

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::core::Value;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::streaming::executor::SortDirection;
use crate::query::executor::streaming::operator_spec::{BlockingSpec, UnarySpec};
use crate::query::executor::streaming::physical_node::PhysicalNode;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

/// Lower a relational plan node into a PhysicalNode tree.
pub fn lower_relational_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, QueryError> {
    match node {
        PlanNodeEnum::Filter(filter_node) => {
            let input_plan = filter_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let condition = filter_node.condition();
            let predicate = contextual_to_expression(condition)?;
            Ok(PhysicalNode::Unary(
                Box::new(input_phys),
                UnarySpec::Filter { predicate },
            ))
        }

        PlanNodeEnum::Project(project_node) => {
            let input_plan = project_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let columns = project_node.columns();
            let output_expressions = yield_columns_to_expressions(columns)?;
            Ok(PhysicalNode::Unary(
                Box::new(input_phys),
                UnarySpec::Project {
                    output_expressions,
                    output_col_names: project_node.col_names().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Limit(limit_node) => {
            let input_plan = limit_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let count = limit_node.count();
            let offset = limit_node.offset();
            let offset = u32::try_from(offset).map_err(|_| {
                QueryError::execution("Limit offset must fit in u32".to_string())
            })?;
            let limit = u32::try_from(count).map_err(|_| {
                QueryError::execution("Limit count must fit in u32".to_string())
            })?;
            Ok(PhysicalNode::Unary(
                Box::new(input_phys),
                UnarySpec::Limit { offset, limit },
            ))
        }

        PlanNodeEnum::Sort(sort_node) => {
            let input_plan = sort_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let sort_items = sort_node.sort_items();
            if sort_items.is_empty() {
                return Ok(input_phys);
            }
            let (sort_expressions, sort_directions) =
                sort_items_to_expressions(sort_items)?;
            Ok(PhysicalNode::Blocking(
                Box::new(input_phys),
                BlockingSpec::Sort {
                    sort_expressions,
                    sort_directions,
                },
            ))
        }

        PlanNodeEnum::Aggregate(agg_node) => {
            let input_plan = agg_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let group_keys = agg_node.group_keys();
            let group_by_expressions: Vec<Expression> = group_keys
                .iter()
                .map(|key| Expression::Variable(key.clone()))
                .collect();
            let agg_functions = agg_node.aggregation_functions();
            let aggregate_functions: Vec<(AggregateFunction, Expression)> = agg_functions
                .iter()
                .map(|func| {
                    let expr = match func {
                        AggregateFunction::Count(Some(field)) => {
                            Expression::Variable(field.clone())
                        }
                        AggregateFunction::Sum(field) => Expression::Variable(field.clone()),
                        AggregateFunction::Avg(field) => Expression::Variable(field.clone()),
                        AggregateFunction::Min(field) => Expression::Variable(field.clone()),
                        AggregateFunction::Max(field) => Expression::Variable(field.clone()),
                        AggregateFunction::Collect(field) => Expression::Variable(field.clone()),
                        AggregateFunction::Count(None) => Expression::Literal(Value::Int(1)),
                        _ => Expression::Literal(Value::Int(1)),
                    };
                    (func.clone(), expr)
                })
                .collect();
            Ok(PhysicalNode::Blocking(
                Box::new(input_phys),
                BlockingSpec::Aggregate {
                    group_by_expressions,
                    aggregate_functions,
                    output_col_names: agg_node.col_names().to_vec(),
                },
            ))
        }

        PlanNodeEnum::Dedup(dedup_node) => {
            let input_plan = dedup_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            Ok(PhysicalNode::Blocking(
                Box::new(input_phys),
                BlockingSpec::Distinct,
            ))
        }

        PlanNodeEnum::TopN(topn_node) => {
            let input_plan = topn_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let sort_items = topn_node.sort_items();
            let (sort_expressions, sort_directions) =
                sort_items_to_expressions(sort_items)?;
            Ok(PhysicalNode::Blocking(
                Box::new(input_phys),
                BlockingSpec::TopN {
                    n: topn_node.limit() as u32,
                    sort_expressions,
                    sort_directions,
                },
            ))
        }

        PlanNodeEnum::Sample(sample_node) => {
            let input_plan = sample_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let count = if sample_node.count() > 0 {
                sample_node.count() as u64
            } else {
                return Err(QueryError::execution(
                    "Sample count must be positive".to_string(),
                ));
            };
            Ok(PhysicalNode::Unary(
                Box::new(input_phys),
                UnarySpec::Sample { count },
            ))
        }

        PlanNodeEnum::Remove(remove_node) => {
            let input_plan = remove_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let remove_items = remove_node.remove_items();
            let columns_to_remove: Vec<String> =
                remove_items.iter().map(|(col, _)| col.clone()).collect();
            Ok(PhysicalNode::Unary(
                Box::new(input_phys),
                UnarySpec::Remove { columns_to_remove },
            ))
        }

        PlanNodeEnum::Assign(node) => {
            let input_plan = node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let assignments: Vec<(String, Expression)> = node
                .assignments()
                .iter()
                .filter_map(|(name, expr)| expr.get_expression().map(|e| (name.clone(), e)))
                .collect();
            Ok(PhysicalNode::Unary(
                Box::new(input_phys),
                UnarySpec::Assign { assignments },
            ))
        }

        PlanNodeEnum::Unwind(node) => {
            let input_plan = node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            Ok(PhysicalNode::Unary(
                Box::new(input_phys),
                UnarySpec::Unwind {
                    unwind_column: node.alias().to_string(),
                },
            ))
        }

        PlanNodeEnum::Materialize(node) => {
            let input_plan = node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            Ok(PhysicalNode::Blocking(
                Box::new(input_phys),
                BlockingSpec::Materialize,
            ))
        }

        PlanNodeEnum::DataCollect(node) => {
            let input_plan = node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            Ok(PhysicalNode::Blocking(
                Box::new(input_phys),
                BlockingSpec::DataCollect,
            ))
        }

        PlanNodeEnum::Window(window_node) => {
            let input_plan = window_node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let window_functions = window_node.window_functions();
            let mut window_exprs = Vec::new();
            let mut partition_by_exprs = Vec::new();
            let mut order_by_exprs = Vec::new();
            let mut order_by_directions = Vec::new();
            for wf in window_functions {
                let window_expr = Expression::WindowFunction {
                    name: wf.name.clone(),
                    args: wf.args.clone(),
                    over_partition_by: wf.partition_by.clone(),
                    over_order_by: wf.order_by.clone(),
                    over_order_desc: wf.order_desc.clone(),
                };
                window_exprs.push(window_expr);
                if partition_by_exprs.is_empty() {
                    partition_by_exprs = wf.partition_by.clone();
                }
                if order_by_exprs.is_empty() {
                    order_by_exprs = wf.order_by.clone();
                    order_by_directions = wf
                        .order_desc
                        .iter()
                        .map(|&desc| {
                            if desc {
                                SortDirection::Descending
                            } else {
                                SortDirection::Ascending
                            }
                        })
                        .collect();
                }
            }
            Ok(PhysicalNode::Blocking(
                Box::new(input_phys),
                BlockingSpec::WindowFunction {
                    window_exprs,
                    partition_by_exprs,
                    order_by_exprs,
                    order_by_directions,
                },
            ))
        }

        PlanNodeEnum::RollUpApply(node) => {
            let input_plan = node.input();
            let input_phys = super::lower_plan_node(input_plan, context)?;
            let rollup_expressions: Vec<Expression> = node
                .compare_cols()
                .iter()
                .map(|c| Expression::Variable(c.clone()))
                .collect();
            Ok(PhysicalNode::Blocking(
                Box::new(input_phys),
                BlockingSpec::RollUpApply { rollup_expressions },
            ))
        }

        _ => Err(QueryError::execution(format!(
            "lowering::relational does not handle node type: {}",
            node.name()
        ))),
    }
}

// ── Helper functions (extracted from builder.rs) ──

fn contextual_to_expression(
    expr: &crate::core::types::expr::ContextualExpression,
) -> Result<Expression, QueryError> {
    expr.get_expression().ok_or_else(|| {
        QueryError::execution("Failed to get expression from ContextualExpression".to_string())
    })
}

fn yield_columns_to_expressions(
    columns: &[crate::core::YieldColumn],
) -> Result<Vec<Expression>, QueryError> {
    columns
        .iter()
        .map(|col| contextual_to_expression(&col.expression))
        .collect()
}

pub fn sort_items_to_expressions(
    items: &[crate::query::planning::plan::core::nodes::operation::sort_node::SortItem],
) -> Result<(Vec<Expression>, Vec<SortDirection>), QueryError> {
    let mut expressions = Vec::new();
    let mut directions = Vec::new();
    for item in items {
        expressions.push(item.expression.clone());
        let direction = match item.direction {
            crate::core::types::graph_schema::OrderDirection::Asc => SortDirection::Ascending,
            crate::core::types::graph_schema::OrderDirection::Desc => SortDirection::Descending,
        };
        directions.push(direction);
    }
    Ok((expressions, directions))
}
