//! RETURN Statement Planner
//!
//! Responsible for planning the execution of the RETURN statement and implementing the projection of the results.

use crate::binder::validation::CypherClauseKind;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::DedupNode;
use crate::planning::plan::core::nodes::graph_operations::window_node::{
    WindowFunctionSpec, WindowNode,
};
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::planning::plan::core::nodes::operation::sample_node::SampleNode;
use crate::planning::plan::core::nodes::AggregateNode;
use crate::planning::plan::logical::logical_nodes::operation::{
    LogicalAggregateNode, LogicalDedupNode, LogicalFilterNode, LogicalProjectNode,
    LogicalSampleNode, LogicalWindowNode,
};
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::SubPlan;
use crate::planning::planner::PlannerError;
use crate::planning::statements::clauses::exists_planner;
use crate::planning::statements::plan_combiner::wrap_logical;
use crate::planning::statements::statement_planner::ClausePlanner;
use crate::QueryContext;
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::expr::expression::ExpressionMeta;
use linkrs_core::types::operators::AggregateFunction;
use linkrs_core::Expression;
use linkrs_core::YieldColumn;
use std::collections::HashSet;
use std::sync::Arc;

pub use crate::planning::plan::core::PlanNodeEnum;

/// RETURN Statement Planner
///
/// Responsible for planning the execution of the RETURN statement and implementing the projection of the results.
#[derive(Debug, Clone)]
pub struct ReturnClausePlanner {
    distinct: bool,
}

impl Default for ReturnClausePlanner {
    fn default() -> Self {
        Self::new()
    }
}

impl ReturnClausePlanner {
    pub fn new() -> Self {
        Self { distinct: false }
    }

    pub fn with_distinct(distinct: bool) -> Self {
        Self { distinct }
    }

    pub fn from_stmt(stmt: &Stmt) -> Self {
        let distinct = extract_distinct_flag(stmt);
        Self::with_distinct(distinct)
    }

    pub fn set_distinct(&mut self, distinct: bool) {
        self.distinct = distinct;
    }
}

fn extract_distinct_flag(stmt: &Stmt) -> bool {
    if let Stmt::Match(match_stmt) = stmt {
        if let Some(return_clause) = &match_stmt.return_clause {
            return return_clause.distinct;
        }
    }
    false
}

fn extract_having_clause(stmt: &Stmt) -> Option<linkrs_core::types::ContextualExpression> {
    if let Stmt::Match(match_stmt) = stmt {
        if let Some(return_clause) = &match_stmt.return_clause {
            return return_clause.having_clause.clone();
        }
    }
    None
}

fn extract_sample_clause(stmt: &Stmt) -> Option<usize> {
    if let Stmt::Match(match_stmt) = stmt {
        if let Some(return_clause) = &match_stmt.return_clause {
            return return_clause.sample.as_ref().map(|s| s.count);
        }
    }
    None
}

fn extract_return_columns(stmt: &Stmt) -> Result<Vec<YieldColumn>, PlannerError> {
    let mut columns = Vec::new();

    if let Stmt::Match(match_stmt) = stmt {
        if let Some(return_clause) = &match_stmt.return_clause {
            for item in &return_clause.items {
                columns.push(
                    crate::planning::statements::projection_util::return_item_to_yield_column(item),
                );
            }
        }
    }

    if columns.is_empty() {
        return Err(PlannerError::PlanGenerationFailed(
            "RETURN clause missing return item".to_string(),
        ));
    }

    Ok(columns)
}

impl ClausePlanner for ReturnClausePlanner {
    fn clause_kind(&self) -> CypherClauseKind {
        CypherClauseKind::Return
    }

    fn transform_clause(
        &self,
        qctx: Arc<QueryContext>,
        stmt: &Stmt,
        input_plan: SubPlan,
    ) -> Result<SubPlan, PlannerError> {
        let mut yield_columns = extract_return_columns(stmt)?;

        let input_node = input_plan.root().as_ref().ok_or_else(|| {
            PlannerError::PlanGenerationFailed(
                "The RETURN clause requires an input plan".to_string(),
            )
        })?;

        // Unified entry for expression-level EXISTS / IN.
        //
        // Plain yield expressions are compiled here and their subqueries are
        // attached to the RETURN Project node; HAVING subqueries attach to
        // the HAVING Filter node. Aggregate / window expression paths run
        // inside blocking operators that do not yet host a subquery executor,
        // so their yield positions are still refused at planning time with a
        // precise error instead of leaking to the runtime "not supported"
        // path.
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names = input_plan
            .root()
            .as_ref()
            .map(|root| root.col_names().to_vec())
            .unwrap_or_default();
        let has_aggregate = yield_columns.iter().any(|col| {
            if let Some(expr_meta) = col.expression.expression() {
                expression_contains_aggregate(expr_meta.inner())
            } else {
                false
            }
        });
        let has_window = yield_columns.iter().any(|col| {
            if let Some(expr_meta) = col.expression.expression() {
                expression_contains_window_function(expr_meta.inner())
            } else {
                false
            }
        });

        let mut id_alloc = exists_planner::SubqueryIdAllocator::new();
        let mut yield_subqueries: Vec<exists_planner::PlannedSubquery> = Vec::new();
        if has_aggregate || has_window {
            for col in &yield_columns {
                if let Some(expr_meta) = col.expression.expression() {
                    exists_planner::check_expression_subqueries(
                        expr_meta.inner(),
                        &qctx,
                        space_id,
                        &space_name,
                        &outer_col_names,
                    )?;
                }
            }
        } else {
            for col in &mut yield_columns {
                let subqueries = exists_planner::plan_contextual_subqueries(
                    &mut col.expression,
                    &qctx,
                    space_id,
                    &space_name,
                    &outer_col_names,
                    &mut id_alloc,
                )?;
                yield_subqueries.extend(subqueries);
            }
        }
        let mut having_subqueries: Vec<exists_planner::PlannedSubquery> = Vec::new();
        let having_expr = extract_having_clause(stmt)
            .map(|mut expr| {
                let subqueries = exists_planner::plan_contextual_subqueries(
                    &mut expr,
                    &qctx,
                    space_id,
                    &space_name,
                    &outer_col_names,
                    &mut id_alloc,
                )?;
                having_subqueries = subqueries;
                Ok::<_, PlannerError>(expr)
            })
            .transpose()?;

        if has_aggregate {
            let (group_keys, agg_functions, agg_aliases, agg_distinct, agg_filters, agg_args) =
                extract_aggregate_info(&yield_columns)?;

            let mut project_columns: Vec<YieldColumn> = yield_columns
                .iter()
                .filter(|col| {
                    if let Some(expr_meta) = col.expression.expression() {
                        !expression_contains_aggregate(expr_meta.inner())
                    } else {
                        false
                    }
                })
                .cloned()
                .collect();

            // Also project the argument expressions from aggregate functions
            // so they are available as input columns for the AggregateExecutor.
            // The guard set is updated on every append: several aggregates
            // often share one argument (e.g. avg(x), max(x)), and appending
            // it twice would emit duplicate output aliases.
            let mut projected_aliases: HashSet<String> =
                project_columns.iter().map(|pc| pc.alias.clone()).collect();
            for col in &yield_columns {
                if let Some(expr_meta) = col.expression.expression() {
                    let inner = expr_meta.inner();
                    if let Expression::Aggregate { args, .. } = inner {
                        for arg in args {
                            let arg_expr_str = arg.to_expression_string();
                            // Only add if not already projected (avoid duplicates)
                            if projected_aliases.insert(arg_expr_str.clone()) {
                                let ctx = col.expression.context();
                                let meta = ExpressionMeta::new(arg.clone());
                                let id = ctx.register_expression(meta);
                                let ctx_expr = ContextualExpression::new(id, ctx.clone());
                                project_columns.push(YieldColumn {
                                    expression: ctx_expr,
                                    alias: arg_expr_str,
                                });
                            }
                        }
                    }
                }
            }

            let project_node = ProjectNode::new(input_node.clone(), project_columns.clone())?
                .with_subqueries(yield_subqueries.clone());
            let project_logical = wrap_logical(&input_plan, |input| {
                LogicalNodeEnum::Project(LogicalProjectNode {
                    id: next_node_id(),
                    input: Some(Box::new(input)),
                    columns: project_columns.clone(),
                    subqueries: yield_subqueries.clone(),
                    has_folded_expressions: false,
                    output_var: None,
                    col_names: project_columns
                        .iter()
                        .map(|col| col.alias.clone())
                        .collect(),
                    column_types:
                        crate::planning::statements::projection_util::project_column_types(
                            &project_columns,
                        ),
                })
            });
            let project_plan = SubPlan::from_dual(
                Some(project_node.into_enum()),
                input_plan.tail,
                project_logical,
            );

            let group_key_exprs = yield_columns
                .iter()
                .filter(|col| {
                    if let Some(expr_meta) = col.expression.expression() {
                        !expression_contains_aggregate(expr_meta.inner())
                    } else {
                        false
                    }
                })
                .map(|col| col.expression.clone())
                .collect::<Vec<_>>();
            let mut aggregate_node = AggregateNode::with_agg_aliases(
                project_plan.root.clone().unwrap(),
                group_keys.clone(),
                agg_functions.clone(),
                agg_aliases,
            )?;
            aggregate_node.set_aggregation_args(agg_args);
            aggregate_node.set_aggregation_distinct(agg_distinct.clone());
            aggregate_node.set_aggregation_filters(agg_filters.clone());
            aggregate_node.set_group_key_exprs(group_key_exprs.clone());
            let logical_agg_args = aggregate_node.aggregation_args().to_vec();

            let mut final_node: PlanNodeEnum = aggregate_node.into_enum();

            // Logical mirror: project → aggregate → [having filter] →
            // [dedup] → [sample].
            let mut logical_root = wrap_logical(&project_plan, |input| {
                LogicalNodeEnum::Aggregate(LogicalAggregateNode {
                    id: next_node_id(),
                    input: Some(Box::new(input)),
                    group_key_exprs: group_key_exprs.clone(),
                    aggregation_functions: agg_functions,
                    aggregation_args: logical_agg_args.clone(),
                    aggregation_distinct: agg_distinct,
                    aggregation_filters: agg_filters,
                    grouping_sets: vec![],
                    output_var: None,
                    col_names: vec![],
                    column_types: vec![],
                })
            });

            // Apply HAVING clause filter if present
            if let Some(having_expr) = having_expr {
                let filter_node = FilterNode::new(final_node.clone(), having_expr.clone())
                    .map_err(|e| {
                        PlannerError::PlanGenerationFailed(format!(
                            "Failed to create FilterNode for HAVING: {}",
                            e
                        ))
                    })?
                    .with_subqueries(having_subqueries);
                final_node = PlanNodeEnum::Filter(filter_node);
                logical_root = logical_root.map(|input| {
                    LogicalNodeEnum::Filter(LogicalFilterNode {
                        id: next_node_id(),
                        input: Some(Box::new(input)),
                        condition: having_expr,
                        output_var: None,
                        col_names: vec![],
                        column_types: vec![],
                    })
                });
            }

            if self.distinct {
                if let Ok(dedup) = DedupNode::new(final_node.clone()) {
                    final_node = dedup.into_enum();
                }
                logical_root = logical_root.map(|input| {
                    LogicalNodeEnum::Dedup(LogicalDedupNode {
                        id: next_node_id(),
                        input: Some(Box::new(input)),
                        output_var: None,
                        col_names: vec![],
                        column_types: vec![],
                    })
                });
            }

            // Apply SAMPLE clause if present
            if let Some(sample_count) = extract_sample_clause(stmt) {
                if let Ok(sample_node) = SampleNode::new(final_node.clone(), sample_count as i64) {
                    final_node = PlanNodeEnum::Sample(sample_node);
                }
                logical_root = logical_root.map(|input| {
                    LogicalNodeEnum::Sample(LogicalSampleNode {
                        id: next_node_id(),
                        input: Some(Box::new(input)),
                        count: sample_count as i64,
                        output_var: None,
                        col_names: vec![],
                        column_types: vec![],
                    })
                });
            }

            Ok(SubPlan::from_dual(
                Some(final_node),
                project_plan.tail,
                logical_root,
            ))
        } else if has_window {
            let window_specs = extract_window_function_info(&yield_columns)?;

            let mut project_columns: Vec<YieldColumn> = yield_columns
                .iter()
                .filter(|col| {
                    if let Some(expr_meta) = col.expression.expression() {
                        !expression_contains_window_function(expr_meta.inner())
                    } else {
                        false
                    }
                })
                .cloned()
                .collect();

            let mut projected_aliases: HashSet<String> =
                project_columns.iter().map(|pc| pc.alias.clone()).collect();
            for col in &yield_columns {
                if let Some(expr_meta) = col.expression.expression() {
                    let inner = expr_meta.inner();
                    if let Expression::WindowFunction { args, .. } = inner {
                        for arg in args {
                            let arg_expr_str = arg.to_expression_string();
                            if projected_aliases.insert(arg_expr_str.clone()) {
                                let ctx = col.expression.context();
                                let meta = ExpressionMeta::new(arg.clone());
                                let id = ctx.register_expression(meta);
                                let ctx_expr = ContextualExpression::new(id, ctx.clone());
                                project_columns.push(YieldColumn {
                                    expression: ctx_expr,
                                    alias: arg_expr_str,
                                });
                            }
                        }
                    }
                }
            }

            let project_node = ProjectNode::new(input_node.clone(), project_columns.clone())?
                .with_subqueries(yield_subqueries.clone());
            let window_node = WindowNode::new(
                project_node.clone().into_enum(),
                window_specs.clone(),
            )
            .map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create WindowNode: {}", e))
            })?;

            let mut final_node = window_node.into_enum();

            // Logical mirror: project → window → [dedup] → [sample].
            let project_logical = wrap_logical(&input_plan, |input| {
                LogicalNodeEnum::Project(LogicalProjectNode {
                    id: next_node_id(),
                    input: Some(Box::new(input)),
                    columns: project_columns.clone(),
                    subqueries: yield_subqueries.clone(),
                    has_folded_expressions: false,
                    output_var: None,
                    col_names: project_columns
                        .iter()
                        .map(|col| col.alias.clone())
                        .collect(),
                    column_types:
                        crate::planning::statements::projection_util::project_column_types(
                            &project_columns,
                        ),
                })
            });
            let mut logical_root = project_logical.map(|input| {
                LogicalNodeEnum::Window(LogicalWindowNode {
                    id: next_node_id(),
                    input: Some(Box::new(input)),
                    window_functions: window_specs,
                    output_var: None,
                    col_names: vec![],
                    column_types: vec![],
                })
            });

            if self.distinct {
                if let Ok(dedup) = DedupNode::new(final_node.clone()) {
                    final_node = dedup.into_enum();
                }
                logical_root = logical_root.map(|input| {
                    LogicalNodeEnum::Dedup(LogicalDedupNode {
                        id: next_node_id(),
                        input: Some(Box::new(input)),
                        output_var: None,
                        col_names: vec![],
                        column_types: vec![],
                    })
                });
            }

            if let Some(sample_count) = extract_sample_clause(stmt) {
                if let Ok(sample_node) = SampleNode::new(final_node.clone(), sample_count as i64) {
                    final_node = PlanNodeEnum::Sample(sample_node);
                }
                logical_root = logical_root.map(|input| {
                    LogicalNodeEnum::Sample(LogicalSampleNode {
                        id: next_node_id(),
                        input: Some(Box::new(input)),
                        count: sample_count as i64,
                        output_var: None,
                        col_names: vec![],
                        column_types: vec![],
                    })
                });
            }

            Ok(SubPlan::from_dual(Some(final_node), None, logical_root))
        } else {
            let project_node = ProjectNode::new(input_node.clone(), yield_columns.clone())?
                .with_subqueries(yield_subqueries.clone());

            let mut final_node = if self.distinct {
                match DedupNode::new(project_node.clone().into_enum()) {
                    Ok(dedup) => dedup.into_enum(),
                    Err(_) => project_node.into_enum(),
                }
            } else {
                project_node.into_enum()
            };

            // Logical mirror: project → [dedup] → [sample].
            let mut logical_root = wrap_logical(&input_plan, |input| {
                LogicalNodeEnum::Project(LogicalProjectNode {
                    id: next_node_id(),
                    input: Some(Box::new(input)),
                    columns: yield_columns.clone(),
                    subqueries: yield_subqueries.clone(),
                    has_folded_expressions: false,
                    output_var: None,
                    col_names: yield_columns.iter().map(|col| col.alias.clone()).collect(),
                    column_types:
                        crate::planning::statements::projection_util::project_column_types(
                            &yield_columns,
                        ),
                })
            });

            if self.distinct {
                logical_root = logical_root.map(|input| {
                    LogicalNodeEnum::Dedup(LogicalDedupNode {
                        id: next_node_id(),
                        input: Some(Box::new(input)),
                        output_var: None,
                        col_names: vec![],
                        column_types: vec![],
                    })
                });
            }

            // Apply SAMPLE clause if present
            if let Some(sample_count) = extract_sample_clause(stmt) {
                if let Ok(sample_node) = SampleNode::new(final_node.clone(), sample_count as i64) {
                    final_node = PlanNodeEnum::Sample(sample_node);
                }
                logical_root = logical_root.map(|input| {
                    LogicalNodeEnum::Sample(LogicalSampleNode {
                        id: next_node_id(),
                        input: Some(Box::new(input)),
                        count: sample_count as i64,
                        output_var: None,
                        col_names: vec![],
                        column_types: vec![],
                    })
                });
            }

            Ok(SubPlan::from_dual(
                Some(final_node),
                input_plan.tail,
                logical_root,
            ))
        }
    }
}

fn expression_contains_aggregate(expr: &linkrs_core::Expression) -> bool {
    use linkrs_core::Expression;
    match expr {
        Expression::Aggregate { .. } => true,
        Expression::Binary { left, right, .. } => {
            expression_contains_aggregate(left) || expression_contains_aggregate(right)
        }
        Expression::Unary { operand, .. } => expression_contains_aggregate(operand),
        Expression::Function { args, .. } => args
            .iter()
            .any(|a| expression_contains_aggregate(a.as_expr())),
        _ => false,
    }
}

fn expression_contains_window_function(expr: &linkrs_core::Expression) -> bool {
    use linkrs_core::Expression;
    match expr {
        Expression::WindowFunction { .. } => true,
        Expression::Binary { left, right, .. } => {
            expression_contains_window_function(left) || expression_contains_window_function(right)
        }
        Expression::Unary { operand, .. } => expression_contains_window_function(operand),
        Expression::Function { args, .. } => args
            .iter()
            .any(|a| expression_contains_window_function(a.as_expr())),
        _ => false,
    }
}

fn extract_window_function_info(
    columns: &[YieldColumn],
) -> Result<Vec<WindowFunctionSpec>, PlannerError> {
    let mut window_specs = Vec::new();
    for col in columns {
        if let Some(expr_meta) = col.expression.expression() {
            let expr = expr_meta.inner();
            if let Expression::WindowFunction {
                name,
                args,
                over_partition_by,
                over_order_by,
                over_order_desc,
            } = expr
            {
                window_specs.push(WindowFunctionSpec {
                    name: name.clone(),
                    args: args.clone(),
                    partition_by: over_partition_by.clone(),
                    order_by: over_order_by.clone(),
                    order_desc: over_order_desc.clone(),
                });
            }
        }
    }
    Ok(window_specs)
}

/// Aggregate extraction result containing group keys, aggregate functions, their aliases, distinct flags, filters, and args
type AggregateExtractionResult = (
    Vec<String>,
    Vec<AggregateFunction>,
    Vec<String>,
    Vec<bool>,
    Vec<Option<Expression>>,
    Vec<Vec<Expression>>,
);

fn extract_aggregate_info(
    columns: &[YieldColumn],
) -> Result<AggregateExtractionResult, PlannerError> {
    let mut group_keys = Vec::new();
    let mut agg_functions = Vec::new();
    let mut agg_aliases = Vec::new();
    let mut agg_distinct = Vec::new();
    let mut agg_filters = Vec::new();
    let mut agg_args = Vec::new();

    for col in columns {
        if let Some(expr_meta) = col.expression.expression() {
            let expr = expr_meta.inner();
            if expression_contains_aggregate(expr) {
                if let Some((agg_func, distinct, filter, args)) = extract_aggregate_function(expr) {
                    agg_functions.push(agg_func);
                    agg_aliases.push(col.alias.clone());
                    agg_distinct.push(distinct);
                    agg_filters.push(filter);
                    agg_args.push(args);
                }
            } else {
                let key = col.alias.clone();
                if !group_keys.contains(&key) {
                    group_keys.push(key);
                }
            }
        }
    }

    Ok((
        group_keys,
        agg_functions,
        agg_aliases,
        agg_distinct,
        agg_filters,
        agg_args,
    ))
}

fn extract_aggregate_function(
    expr: &linkrs_core::Expression,
) -> Option<(AggregateFunction, bool, Option<Expression>, Vec<Expression>)> {
    use linkrs_core::Expression;
    match expr {
        Expression::Aggregate {
            func,
            distinct,
            filter,
            args,
            ..
        } => Some((
            *func,
            *distinct,
            filter.as_ref().map(|f| f.as_ref().clone()),
            args.clone(),
        )),
        Expression::Binary { left, right, .. } => {
            extract_aggregate_function(left).or_else(|| extract_aggregate_function(right))
        }
        Expression::Unary { operand, .. } => extract_aggregate_function(operand),
        Expression::Function { args, .. } => args
            .iter()
            .find_map(|a| extract_aggregate_function(a.as_expr())),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::arc_with_non_send_sync)]
#[cfg(test)]
mod tests;
