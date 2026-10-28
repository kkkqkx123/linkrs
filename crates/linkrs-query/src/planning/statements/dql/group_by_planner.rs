//! GroupBy Operation Planner
//!
//! Query planning for statements that involve the GROUP BY clause

use crate::binder::BoundStatement;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{AggregateNode, ProjectNode, ScanVerticesNode};
use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError};
use crate::planning::statements::clauses::exists_planner::to_contextual;
use crate::planning::statements::plan_combiner::wrap_logical_project;
use crate::QueryContext;
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::types::expr::Expression;
use linkrs_core::types::expr::ExpressionMeta;
use linkrs_core::types::operators::AggregateFunction;
use std::sync::Arc;

/// GroupBy Operation Planner
/// Responsible for converting GROUP BY statements into execution plans.
#[derive(Debug, Clone)]
pub struct GroupByPlanner;

impl GroupByPlanner {
    /// Create a new GroupBy planner.
    pub fn new() -> Self {
        Self
    }
}

impl Planner for GroupByPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let group_by = match bound {
            BoundStatement::GroupBy(g) => g,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "GroupByPlanner requires the GroupBy statement.".to_string(),
                ));
            }
        };

        let expr_ctx = Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let mut group_keys = Vec::new();
        let mut group_key_exprs = Vec::new();
        for k in &group_by.keys {
            match crate::binder::expr_converter::bound_expr_to_contextual(k, &expr_ctx) {
                Ok(ctx_expr) => {
                    group_keys.push(ctx_expr.to_expression_string());
                    group_key_exprs.push(ctx_expr);
                }
                Err(_) => {
                    group_keys.push("_".to_string());
                    group_key_exprs.push(to_contextual(
                        Expression::Variable("_".to_string()),
                        &expr_ctx,
                    ));
                }
            }
        }

        let mut aggregation_functions = Vec::new();
        let mut aggregation_args = Vec::new();
        let mut agg_aliases = Vec::new();

        for agg in &group_by.aggregates {
            let func = match agg.function_name.to_uppercase().as_str() {
                "COUNT" => AggregateFunction::Count,
                "SUM" => AggregateFunction::Sum,
                "AVG" => AggregateFunction::Avg,
                "MAX" => AggregateFunction::Max,
                "MIN" => AggregateFunction::Min,
                "COLLECT" => AggregateFunction::Collect,
                "STD" => AggregateFunction::Std,
                "STDDEV" => AggregateFunction::StddevPop,
                "VARIANCE" => AggregateFunction::Variance,
                "PRODUCT" => AggregateFunction::Product,
                _ => AggregateFunction::Count,
            };
            aggregation_functions.push(func);

            let args: Vec<Expression> = agg
                .arguments
                .iter()
                .map(|arg| {
                    let expr_ctx = Arc::new(
                        linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
                    );
                    crate::binder::expr_converter::bound_expr_to_contextual(arg, &expr_ctx)
                        .map(|ctx| {
                            ctx.expression()
                                .map(|m| m.inner().clone())
                                .unwrap_or_else(|| Expression::Variable("_".to_string()))
                        })
                        .unwrap_or_else(|_| Expression::Variable("_".to_string()))
                })
                .collect();
            aggregation_args.push(args);

            agg_aliases.push(
                agg.alias
                    .clone()
                    .unwrap_or_else(|| format!("agg_{}", agg.function_name)),
            );
        }

        // Build the standalone ScanVertices -> Project adapter so a
        // top-level GROUP BY aggregates over every vertex, mirroring the
        // legacy transform path. When the GROUP BY is a pipe stage,
        // PipePlanner replaces this adapter with the piped rows.
        let (input_enum, input_tail, input_logical) = self.build_standalone_input(
            &group_keys,
            &aggregation_functions,
            &aggregation_args,
            qctx,
        )?;

        let mut aggregate_node = AggregateNode::with_agg_aliases(
            input_enum.clone(),
            group_keys.clone(),
            aggregation_functions,
            agg_aliases,
        )
        .map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create AggregateNode: {}", e))
        })?;
        aggregate_node.set_aggregation_args(aggregation_args);
        aggregate_node.set_group_key_exprs(group_key_exprs.clone());

        let logical_root = aggregate_mirror(&aggregate_node, input_logical, group_key_exprs);
        let sub_plan = SubPlan::from_dual(
            Some(PlanNodeEnum::Aggregate(aggregate_node)),
            Some(input_tail),
            Some(logical_root),
        );
        Ok(sub_plan)
    }
}

impl GroupByPlanner {
    /// Build the input plan for a standalone GROUP BY statement.
    ///
    /// The input is planned as ScanVertices -> Project(v.<property> AS <name>),
    /// so the aggregate evaluates its group keys and aggregate function fields
    /// against flat property columns. When the GROUP BY appears on the right
    /// side of a pipe, PipePlanner replaces this adapter with the piped rows.
    ///
    /// Returns the physical input root, the physical tail, and the native
    /// logical mirror of the adapter.
    fn build_standalone_input(
        &self,
        group_keys: &[String],
        aggregation_functions: &[AggregateFunction],
        aggregation_args: &[Vec<Expression>],
        qctx: Arc<QueryContext>,
    ) -> Result<(PlanNodeEnum, PlanNodeEnum, LogicalNodeEnum), PlannerError> {
        let space_name = qctx.space_name().unwrap_or_default();

        // Collect the property names referenced by the group keys and the
        // aggregate function fields.
        let mut properties: Vec<String> = group_keys.to_vec();
        for (i, func) in aggregation_functions.iter().enumerate() {
            if let Some(field) = Self::aggregate_field(
                func,
                aggregation_args.get(i).map(|a| a.as_slice()).unwrap_or(&[]),
            ) {
                properties.push(field);
            }
        }
        properties.sort();
        properties.dedup();

        let mut scan_node = ScanVerticesNode::new(0, &space_name);
        scan_node.set_col_names(vec!["v".to_string()]);
        scan_node.set_projected_properties(properties.clone());
        let scan_enum = PlanNodeEnum::ScanVertices(scan_node);

        // Project the needed vertex properties into flat columns.
        let expr_ctx = Arc::new(
            linkrs_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
        );
        let mut yield_columns = Vec::new();
        for property in &properties {
            let expression = Expression::Property {
                object: Box::new(Expression::Variable("v".to_string())),
                property: property.clone(),
            };
            let expr_id = expr_ctx.register_expression(ExpressionMeta::new(expression));
            let ctx_expr = ContextualExpression::new(expr_id, expr_ctx.clone());
            yield_columns.push(linkrs_core::YieldColumn {
                expression: ctx_expr,
                alias: property.clone(),
            });
        }

        let project_node =
            ProjectNode::new(scan_enum.clone(), yield_columns.clone()).map_err(|e| {
                PlannerError::PlanGenerationFailed(format!("Failed to create ProjectNode: {}", e))
            })?;
        let project_enum = PlanNodeEnum::Project(project_node);

        let logical_scan = LogicalNodeEnum::ScanVertices(LogicalScanVerticesNode {
            id: next_node_id(),
            space_id: 0,
            space_name: space_name.clone(),
            tag: None,
            expression: None,
            limit: None,
            projected_properties: properties,
            index_hint: None,
            estimated_cardinality: None,
            output_var: None,
            col_names: vec!["v".to_string()],
            column_types: vec![],
        });
        let logical_project = wrap_logical_project(
            logical_scan,
            yield_columns,
            project_enum.col_names().to_vec(),
        );

        Ok((project_enum, scan_enum, logical_project))
    }

    /// Return the input field name referenced by an aggregate function, if any.
    /// Extracts from the first argument expression (the field being aggregated).
    fn aggregate_field(func: &AggregateFunction, args: &[Expression]) -> Option<String> {
        match func {
            AggregateFunction::Count => None,
            _ => {
                if let Some(Expression::Variable(field)) = args.first() {
                    Some(field.clone())
                } else {
                    None
                }
            }
        }
    }
}

impl Default for GroupByPlanner {
    fn default() -> Self {
        Self::new()
    }
}

/// Mirror a physical aggregate over the standalone logical input.
///
/// The caller supplies lossless group key identities; the remaining aggregate
/// payload is read back from the physical node so both trees describe the
/// same operator.
fn aggregate_mirror(
    aggregate_node: &AggregateNode,
    input: LogicalNodeEnum,
    group_key_exprs: Vec<ContextualExpression>,
) -> LogicalNodeEnum {
    LogicalNodeEnum::Aggregate(LogicalAggregateNode {
        id: next_node_id(),
        input: Some(Box::new(input)),
        group_key_exprs,
        aggregation_functions: aggregate_node.aggregation_functions().to_vec(),
        aggregation_args: aggregate_node.aggregation_args().to_vec(),
        aggregation_distinct: aggregate_node.aggregation_distinct().to_vec(),
        aggregation_filters: aggregate_node.aggregation_filters().to_vec(),
        grouping_sets: aggregate_node.grouping_sets().to_vec(),
        output_var: None,
        col_names: aggregate_node.col_names().to_vec(),
        column_types: vec![],
    })
}
