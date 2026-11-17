//! GroupBy Operation Planner
//!
//! Query planning for statements that involve the GROUP BY clause

use crate::binder::BoundStatement;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{
    AggregateNode, FilterNode, ProjectNode, ScanVerticesNode,
};
use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
use crate::planning::plan::logical::logical_nodes::operation::{
    LogicalAggregateNode, LogicalFilterNode,
};
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
        let mut agg_distinct = Vec::new();
        let mut agg_filters: Vec<Option<Expression>> = Vec::new();

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
            agg_distinct.push(agg.distinct);
            // A filter that cannot round-trip is a planning error, never a
            // silent drop: an unfiltered aggregate would emit extra rows.
            let filter = agg
                .filter
                .as_ref()
                .map(|f| {
                    crate::binder::expr_converter::bound_expr_to_contextual(f, &expr_ctx)
                        .map(|ctx_expr| {
                            ctx_expr
                                .expression()
                                .map(|m| m.inner().clone())
                                .unwrap_or_else(|| Expression::Variable("_".to_string()))
                        })
                        .map_err(PlannerError::PlanGenerationFailed)
                })
                .transpose()?;
            agg_filters.push(filter);
        }

        // Build the standalone ScanVertices -> Project adapter so a
        // top-level GROUP BY aggregates over every vertex. When the GROUP BY is a pipe stage,
        // PipePlanner replaces this adapter with the piped rows.
        let (input_enum, input_tail, input_logical) = self.build_standalone_input(
            &group_keys,
            &aggregation_functions,
            &aggregation_args,
            &agg_filters,
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
        aggregate_node.set_aggregation_distinct(agg_distinct);
        aggregate_node.set_aggregation_filters(agg_filters);
        aggregate_node.set_group_key_exprs(group_key_exprs.clone());

        let mut physical_root = PlanNodeEnum::Aggregate(aggregate_node);
        let mut logical_root = match &physical_root {
            PlanNodeEnum::Aggregate(aggregate_node) => {
                aggregate_mirror(aggregate_node, input_logical, group_key_exprs)
            }
            _ => unreachable!("group-by root is always an Aggregate node"),
        };
        // HAVING filters aggregated rows above the aggregation, mirroring
        // the RETURN-path HAVING handling.
        if let Some(ref having) = group_by.having {
            let condition =
                crate::binder::expr_converter::bound_expr_to_contextual(having, &expr_ctx)
                    .map_err(PlannerError::PlanGenerationFailed)?;
            let filter_node =
                FilterNode::new(physical_root.clone(), condition.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode for HAVING: {}",
                        e
                    ))
                })?;
            physical_root = PlanNodeEnum::Filter(filter_node);
            logical_root = LogicalNodeEnum::Filter(LogicalFilterNode {
                id: next_node_id(),
                input: Some(Box::new(logical_root)),
                condition,
                output_var: None,
                col_names: vec![],
                column_types: vec![],
            });
        }
        let sub_plan =
            SubPlan::from_dual(Some(physical_root), Some(input_tail), Some(logical_root));
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
        aggregation_filters: &[Option<Expression>],
        qctx: Arc<QueryContext>,
    ) -> Result<(PlanNodeEnum, PlanNodeEnum, LogicalNodeEnum), PlannerError> {
        let space_name = qctx.space_name().unwrap_or_default();

        // Collect the property names referenced by the group keys, the
        // aggregate function fields, and the per-aggregate FILTER
        // predicates. A filter evaluates against the adapter output, so
        // every column it names must be projected here.
        let mut properties: Vec<String> = group_keys.to_vec();
        for (i, func) in aggregation_functions.iter().enumerate() {
            if let Some(field) = Self::aggregate_field(
                func,
                aggregation_args.get(i).map(|a| a.as_slice()).unwrap_or(&[]),
            ) {
                properties.push(field);
            }
            if let Some(filter) = aggregation_filters.get(i).and_then(|f| f.as_ref()) {
                Self::collect_filter_columns(filter, &mut properties);
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

    /// Collect the input column names named by an aggregate FILTER
    /// predicate: bare variables resolve by name, property accesses by
    /// their property name (the standalone adapter flattens `v.<prop>` to
    /// `<prop>`).
    fn collect_filter_columns(expr: &Expression, out: &mut Vec<String>) {
        match expr {
            Expression::Variable(name) => out.push(name.clone()),
            Expression::Property { property, .. }
            | Expression::TagProperty { property, .. }
            | Expression::EdgeProperty { property, .. } => out.push(property.clone()),
            Expression::Binary { left, right, .. } => {
                Self::collect_filter_columns(left, out);
                Self::collect_filter_columns(right, out);
            }
            Expression::Unary { operand, .. } => Self::collect_filter_columns(operand, out),
            Expression::Function { args, .. } => {
                for arg in args {
                    Self::collect_filter_columns(arg.as_expr(), out);
                }
            }
            Expression::Aggregate { args, filter, .. } => {
                for arg in args {
                    Self::collect_filter_columns(arg, out);
                }
                if let Some(filter) = filter {
                    Self::collect_filter_columns(filter, out);
                }
            }
            Expression::List(items) => {
                for item in items {
                    Self::collect_filter_columns(item, out);
                }
            }
            Expression::Map(pairs) => {
                for (_, value) in pairs {
                    Self::collect_filter_columns(value, out);
                }
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(test) = test_expr {
                    Self::collect_filter_columns(test, out);
                }
                for (condition, value) in conditions {
                    Self::collect_filter_columns(condition, out);
                    Self::collect_filter_columns(value, out);
                }
                if let Some(default) = default {
                    Self::collect_filter_columns(default, out);
                }
            }
            Expression::TypeCast { expression, .. } => {
                Self::collect_filter_columns(expression, out)
            }
            Expression::Subscript { collection, index } => {
                Self::collect_filter_columns(collection, out);
                Self::collect_filter_columns(index, out);
            }
            _ => {}
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binder::bound::{BoundAggregateCall, BoundExpression, BoundGroupByStatement};
    use crate::planning::context::PlanContext;
    use linkrs_core::types::operators::BinaryOperator;
    use linkrs_core::types::semantic::ValueType;
    use linkrs_core::{DataType, Value};

    fn test_query_context() -> Arc<QueryContext> {
        let rctx = Arc::new(crate::QueryRequestContext::new("GROUP BY".to_string()));
        let space_info = linkrs_core::types::SpaceInfo {
            space_id: 1,
            space_name: "test".to_string(),
            ..Default::default()
        };
        Arc::new(
            crate::QueryContext::builder(rctx)
                .with_space_info(space_info)
                .build(),
        )
    }

    fn count_agg(alias: &str, filter: Option<BoundExpression>) -> BoundAggregateCall {
        BoundAggregateCall {
            function_name: "COUNT".to_string(),
            arguments: vec![],
            distinct: false,
            alias: Some(alias.to_string()),
            filter: filter.map(Box::new),
            return_type: ValueType::Int,
        }
    }

    fn plan_group_by(group_by: BoundGroupByStatement) -> Result<SubPlan, PlannerError> {
        let bound = BoundStatement::GroupBy(group_by);
        let qctx = test_query_context();
        let ctx = PlanContext::without_metadata(&bound, qctx);
        GroupByPlanner::new().plan_bound(&ctx)
    }

    fn city_group_by(
        having: Option<BoundExpression>,
        filter: Option<BoundExpression>,
    ) -> BoundGroupByStatement {
        BoundGroupByStatement {
            keys: vec![BoundExpression::Variable(
                "city".to_string(),
                DataType::Unknown,
            )],
            aggregates: vec![count_agg("c", filter)],
            having,
        }
    }

    #[test]
    fn having_wraps_aggregate_on_both_trees() {
        let having = BoundExpression::BinaryOp {
            left: Box::new(BoundExpression::Variable(
                "c".to_string(),
                DataType::Unknown,
            )),
            op: BinaryOperator::GreaterThan,
            right: Box::new(BoundExpression::Literal(Value::Int(1), DataType::Int)),
            return_type: DataType::Bool,
        };
        let plan = plan_group_by(city_group_by(Some(having), None)).expect("group by plans");
        let physical_root = plan.root.as_ref().expect("physical root");
        match physical_root {
            PlanNodeEnum::Filter(_) => {}
            other => panic!("physical root must be the HAVING filter, got {:?}", other),
        }
        let logical_root = plan.logical_root.as_ref().expect("logical root");
        match logical_root {
            LogicalNodeEnum::Filter(_) => {}
            other => panic!("logical root must be the HAVING filter, got {:?}", other),
        }
    }

    #[test]
    fn aggregate_filter_reaches_the_aggregate_node() {
        let filter = BoundExpression::BinaryOp {
            left: Box::new(BoundExpression::Variable(
                "age".to_string(),
                DataType::Unknown,
            )),
            op: BinaryOperator::GreaterThan,
            right: Box::new(BoundExpression::Literal(Value::Int(30), DataType::Int)),
            return_type: DataType::Bool,
        };
        let plan = plan_group_by(city_group_by(None, Some(filter))).expect("group by plans");
        let physical_root = plan.root.as_ref().expect("physical root");
        let PlanNodeEnum::Aggregate(agg) = physical_root else {
            panic!("physical root must be Aggregate, got {:?}", physical_root);
        };
        assert!(
            agg.aggregation_filters().iter().any(|f| f.is_some()),
            "aggregate FILTER must reach the physical node"
        );
        let logical_root = plan.logical_root.as_ref().expect("logical root");
        let LogicalNodeEnum::Aggregate(agg) = logical_root else {
            panic!("logical root must be Aggregate, got {:?}", logical_root);
        };
        assert!(
            agg.aggregation_filters.iter().any(|f| f.is_some()),
            "aggregate FILTER must reach the logical node"
        );
    }
}
