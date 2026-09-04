//! GroupBy Operation Planner
//!
//! Query planning for statements that involve the GROUP BY clause

use crate::binder::BoundStatement;
use crate::parser::ast::{GroupingType, Stmt};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::{
    AggregateNode, FilterNode, ProjectNode, ScanVerticesNode,
};
use crate::planning::plan::logical::logical_nodes::access::LogicalScanVerticesNode;
use crate::planning::plan::logical::logical_nodes::operation::LogicalAggregateNode;
use crate::planning::plan::logical::LogicalNodeEnum;
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::planning::statements::clauses::exists_planner;
use crate::planning::statements::clauses::exists_planner::to_contextual;
use crate::planning::statements::plan_combiner::{
    logical_start_root, wrap_logical_filter, wrap_logical_project,
};
use crate::QueryContext;
use graphdb_core::types::expr::contextual::ContextualExpression;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::expr::ExpressionMeta;
use graphdb_core::types::operators::AggregateFunction;
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

    /// Extract the aggregate functions from the expression.
    ///
    /// Recursively traverse the expression tree and collect all aggregate functions.
    /// Refer to the implementation of ExpressionUtils::collectAll in nebula-graph.
    fn extract_aggregate_functions(
        &self,
        expr: &ContextualExpression,
    ) -> Vec<(AggregateFunction, bool, Option<Expression>)> {
        let expr_meta = match expr.expression() {
            Some(e) => e,
            None => return Vec::new(),
        };
        let inner_expr = expr_meta.inner();
        let mut functions = Vec::new();
        self.collect_aggregate_functions_recursive(inner_expr, &mut functions);
        functions
    }

    /// Auxiliary method for recursively collecting aggregate functions
    fn collect_aggregate_functions_recursive(
        &self,
        expr: &Expression,
        functions: &mut Vec<(AggregateFunction, bool, Option<Expression>)>,
    ) {
        match expr {
            Expression::Aggregate {
                func,
                distinct,
                filter,
                ..
            } => {
                functions.push((
                    *func,
                    *distinct,
                    filter.as_ref().map(|f| f.as_ref().clone()),
                ));
            }
            Expression::Binary { left, right, .. } => {
                self.collect_aggregate_functions_recursive(left, functions);
                self.collect_aggregate_functions_recursive(right, functions);
            }
            Expression::Unary { operand, .. } => {
                self.collect_aggregate_functions_recursive(operand, functions);
            }
            Expression::Function { args, .. } => {
                for arg in args {
                    self.collect_aggregate_functions_recursive(arg, functions);
                }
            }
            Expression::List(items) => {
                for item in items {
                    self.collect_aggregate_functions_recursive(item, functions);
                }
            }
            Expression::Map(pairs) => {
                for (_, value) in pairs {
                    self.collect_aggregate_functions_recursive(value, functions);
                }
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(test) = test_expr {
                    self.collect_aggregate_functions_recursive(test, functions);
                }
                for (when_expr, then_expr) in conditions {
                    self.collect_aggregate_functions_recursive(when_expr, functions);
                    self.collect_aggregate_functions_recursive(then_expr, functions);
                }
                if let Some(def) = default {
                    self.collect_aggregate_functions_recursive(def, functions);
                }
            }
            Expression::Property { object, .. } => {
                self.collect_aggregate_functions_recursive(object, functions);
            }
            Expression::StructField { base, .. } => {
                self.collect_aggregate_functions_recursive(base, functions);
            }
            Expression::Subscript { collection, index } => {
                self.collect_aggregate_functions_recursive(collection, functions);
                self.collect_aggregate_functions_recursive(index, functions);
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                self.collect_aggregate_functions_recursive(collection, functions);
                if let Some(s) = start {
                    self.collect_aggregate_functions_recursive(s, functions);
                }
                if let Some(e) = end {
                    self.collect_aggregate_functions_recursive(e, functions);
                }
            }
            Expression::Path(items) => {
                for item in items {
                    self.collect_aggregate_functions_recursive(item, functions);
                }
            }
            Expression::TypeCast { expression, .. } => {
                self.collect_aggregate_functions_recursive(expression, functions);
            }
            Expression::ListComprehension {
                source,
                filter,
                map,
                ..
            } => {
                self.collect_aggregate_functions_recursive(source, functions);
                if let Some(f) = filter {
                    self.collect_aggregate_functions_recursive(f, functions);
                }
                if let Some(m) = map {
                    self.collect_aggregate_functions_recursive(m, functions);
                }
            }
            Expression::LabelTagProperty { tag, .. } => {
                self.collect_aggregate_functions_recursive(tag, functions);
            }
            Expression::Predicate { args, .. } => {
                for arg in args {
                    self.collect_aggregate_functions_recursive(arg, functions);
                }
            }
            Expression::Reduce {
                initial,
                source,
                mapping,
                ..
            } => {
                self.collect_aggregate_functions_recursive(initial, functions);
                self.collect_aggregate_functions_recursive(source, functions);
                self.collect_aggregate_functions_recursive(mapping, functions);
            }
            Expression::PathBuild(items) => {
                for item in items {
                    self.collect_aggregate_functions_recursive(item, functions);
                }
            }
            Expression::Literal(_)
            | Expression::Variable(_)
            | Expression::Label(_)
            | Expression::TagProperty { .. }
            | Expression::EdgeProperty { .. }
            | Expression::Parameter(_)
            | Expression::SessionVariable(_)
            | Expression::Vector(_)
            | Expression::Exists { .. }
            | Expression::In { .. }
            | Expression::WindowFunction { .. } => {}
        }
    }
}

impl Planner for GroupByPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let group_by_stmt = match validated.stmt() {
            Stmt::GroupBy(group_by_stmt) => group_by_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "GroupByPlanner requires the GroupBy statement.".to_string(),
                ));
            }
        };

        // The group keys resolve against the input columns by name, e.g.
        // GROUP BY city produces the key "city".
        let num_group_items = group_by_stmt.group_items.len();
        let group_keys: Vec<String> = group_by_stmt
            .group_items
            .iter()
            .map(|item| item.to_expression_string())
            .collect();

        // Unified entry for expression-level EXISTS / IN. GROUP BY yield
        // expressions run inside the blocking aggregate operator (no subquery
        // executor yet), so they are refused at planning time with a precise
        // error; HAVING subqueries are compiled here and attached to the
        // HAVING Filter node.
        let space_id = qctx.space_id().unwrap_or(1);
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let outer_col_names = group_keys.clone();
        for item in &group_by_stmt.yield_clause.items {
            if let Some(expr_meta) = item.expression.expression() {
                exists_planner::check_expression_subqueries(
                    expr_meta.inner(),
                    &qctx,
                    space_id,
                    &space_name,
                    &outer_col_names,
                )?;
            }
        }
        let mut having_subqueries: Vec<exists_planner::PlannedSubquery> = Vec::new();
        let having_clause = group_by_stmt.having_clause.clone().map(|mut expr| {
            let subqueries = exists_planner::plan_contextual_subqueries(
                &mut expr,
                &qctx,
                space_id,
                &space_name,
                &outer_col_names,
                &mut exists_planner::SubqueryIdAllocator::new(),
            )?;
            having_subqueries = subqueries;
            Ok::<_, PlannerError>(expr)
        });
        let having_clause = match having_clause {
            Some(Ok(expr)) => Some(expr),
            Some(Err(error)) => return Err(error),
            None => None,
        };

        // Extract the aggregate functions with distinct flags, filters, and args
        let mut aggregation_functions = Vec::new();
        let mut aggregation_args = Vec::new();
        let mut aggregation_distinct = Vec::new();
        let mut aggregation_filters = Vec::new();
        for item in &group_by_stmt.yield_clause.items {
            let funcs = self.extract_aggregate_functions(&item.expression);
            for (func, distinct, filter) in funcs {
                aggregation_functions.push(func);
                aggregation_distinct.push(distinct);
                aggregation_filters.push(filter);
            }
            // Also extract the args from the Expression::Aggregate nodes
            if let Some(expr_meta) = item.expression.expression() {
                Self::collect_aggregate_args_recursive(expr_meta.inner(), &mut aggregation_args);
            }
        }

        // Build the input plan. A standalone GROUP BY aggregates over every
        // vertex of the current space; when the GROUP BY is the right side of
        // a pipe, PipePlanner replaces this adapter with the piped rows.
        let (input_enum, input_tail, mut current_logical) = self.build_standalone_input(
            validated,
            &group_keys,
            &aggregation_functions,
            &aggregation_args,
            qctx,
        )?;

        // Generate grouping sets from GroupingType
        let grouping_sets = match &group_by_stmt.grouping_type {
            GroupingType::Standard => Vec::new(),
            GroupingType::Rollup(_) => {
                // ROLLUP(a, b, c) -> (a,b,c), (a,b), (a), ()
                let mut sets: Vec<Vec<String>> = Vec::new();
                for i in (0..=num_group_items).rev() {
                    sets.push(group_keys[0..i].to_vec());
                }
                sets
            }
            GroupingType::Cube(_) => {
                // CUBE(a, b) -> (a,b), (a), (b), ()
                let mut sets: Vec<Vec<String>> = Vec::new();
                for mask in 0..(1u32 << num_group_items) {
                    let mut set = Vec::new();
                    for (i, _) in group_keys.iter().enumerate().take(num_group_items) {
                        if mask & (1 << i) != 0 {
                            set.push(group_keys[i].clone());
                        }
                    }
                    // Sort by original position for deterministic order: larger sets first
                    if !set.is_empty() {
                        sets.push(set);
                    }
                }
                sets.push(Vec::new());
                // Sort descending by number of keys
                sets.sort_by_key(|b| std::cmp::Reverse(b.len()));
                sets.dedup();
                sets
            }
            GroupingType::GroupingSets(sets) => {
                // GROUPING SETS preserves the user's explicit sets
                // Convert the expressions to string keys using their indices in group_items
                sets.iter()
                    .map(|exprs| {
                        exprs
                            .iter()
                            .filter_map(|e| {
                                // Find the expression's index in the original group_items and use the corresponding key name
                                let expr_str = e.expression().map(|m| m.inner().to_string());
                                group_by_stmt
                                    .group_items
                                    .iter()
                                    .position(|gi| {
                                        gi.expression().map(|m| m.inner().to_string()) == expr_str
                                    })
                                    .map(|idx| format!("group_key_{}", idx))
                            })
                            .collect()
                    })
                    .filter(|set: &Vec<String>| !set.is_empty() || sets.is_empty())
                    .collect()
            }
        };

        // Create an aggregate node.
        let mut aggregate_node = AggregateNode::new(
            input_enum.clone(),
            group_keys.clone(),
            aggregation_functions,
        )
        .map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create AggregateNode: {}", e))
        })?;
        aggregate_node.set_aggregation_args(aggregation_args);
        aggregate_node.set_aggregation_distinct(aggregation_distinct);
        aggregate_node.set_aggregation_filters(aggregation_filters);
        aggregate_node.set_grouping_sets(grouping_sets);

        let mut final_node = PlanNodeEnum::Aggregate(aggregate_node);
        if let PlanNodeEnum::Aggregate(ref aggregate) = final_node {
            current_logical = aggregate_mirror(
                aggregate,
                current_logical,
                group_keys
                    .iter()
                    .map(|key| {
                        to_contextual(Expression::Variable(key.clone()), validated.expr_context())
                    })
                    .collect(),
            );
        }

        // If there is a HAVING clause, add a FilterNode.
        if let Some(having_expr) = having_clause {
            let filter_node = FilterNode::new(final_node.clone(), having_expr.clone())
                .map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?
                .with_subqueries(having_subqueries);
            final_node = PlanNodeEnum::Filter(filter_node);
            current_logical = wrap_logical_filter(
                current_logical,
                having_expr,
                final_node.col_names().to_vec(),
            );
        }

        // Create a SubPlan
        let sub_plan = SubPlan {
            root: Some(final_node),
            tail: Some(input_tail),
            logical_root: Some(current_logical),
        };

        Ok(sub_plan)
    }

    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let metadata = ctx.metadata;
        let validated = ctx.validated;
        let _ = (&bound, &qctx, &metadata, &validated);
        let group_by = match bound {
            BoundStatement::GroupBy(g) => g,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "GroupByPlanner requires the GroupBy statement.".to_string(),
                ));
            }
        };

        let group_keys: Vec<String> = group_by
            .keys
            .iter()
            .map(|k| {
                let expr_ctx = Arc::new(
                    graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
                );
                crate::binder::expr_converter::bound_expr_to_contextual(k, &expr_ctx)
                    .map(|ctx| ctx.to_expression_string())
                    .unwrap_or_else(|_| "_".to_string())
            })
            .collect();

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
                        graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
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

        let start_node = crate::planning::plan::core::nodes::StartNode::new();
        let input_enum = PlanNodeEnum::Start(start_node.clone());

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

        let logical_root = {
            let expr_ctx = Arc::new(
                graphdb_core::types::expr::expression_context::ExpressionAnalysisContext::new(),
            );
            let group_key_exprs = group_keys
                .iter()
                .map(|key| to_contextual(Expression::Variable(key.clone()), &expr_ctx))
                .collect();
            aggregate_mirror(&aggregate_node, logical_start_root(), group_key_exprs)
        };
        let sub_plan = SubPlan {
            root: Some(PlanNodeEnum::Aggregate(aggregate_node)),
            tail: Some(input_enum),
            logical_root: Some(logical_root),
        };
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::GroupBy(_))
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
        validated: &ValidatedStatement,
        group_keys: &[String],
        aggregation_functions: &[AggregateFunction],
        aggregation_args: &[Vec<Expression>],
        qctx: Arc<QueryContext>,
    ) -> Result<(PlanNodeEnum, PlanNodeEnum, LogicalNodeEnum), PlannerError> {
        let space_name = qctx
            .space_name()
            .or_else(|| validated.validation_info.semantic_info.space_name.clone())
            .unwrap_or_default();

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
        let expr_ctx = validated.expr_context();
        let mut yield_columns = Vec::new();
        for property in &properties {
            let expression = Expression::Property {
                object: Box::new(Expression::Variable("v".to_string())),
                property: property.clone(),
            };
            let expr_id = expr_ctx.register_expression(ExpressionMeta::new(expression));
            let ctx_expr = ContextualExpression::new(expr_id, expr_ctx.clone());
            yield_columns.push(graphdb_core::YieldColumn {
                expression: ctx_expr,
                alias: property.clone(),
                is_matched: false,
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

    /// Recursively collect args from Expression::Aggregate nodes in parallel
    /// with the `extract_aggregate_functions` traversal.
    fn collect_aggregate_args_recursive(expr: &Expression, args_out: &mut Vec<Vec<Expression>>) {
        match expr {
            Expression::Aggregate { args, .. } => {
                args_out.push(args.clone());
            }
            Expression::Binary { left, right, .. } => {
                Self::collect_aggregate_args_recursive(left, args_out);
                Self::collect_aggregate_args_recursive(right, args_out);
            }
            Expression::Unary { operand, .. } => {
                Self::collect_aggregate_args_recursive(operand, args_out);
            }
            Expression::Function { args, .. } => {
                for arg in args {
                    Self::collect_aggregate_args_recursive(arg, args_out);
                }
            }
            Expression::List(items) => {
                for item in items {
                    Self::collect_aggregate_args_recursive(item, args_out);
                }
            }
            Expression::Map(pairs) => {
                for (_, value) in pairs {
                    Self::collect_aggregate_args_recursive(value, args_out);
                }
            }
            Expression::Case {
                test_expr,
                conditions,
                default,
            } => {
                if let Some(test) = test_expr {
                    Self::collect_aggregate_args_recursive(test, args_out);
                }
                for (when_expr, then_expr) in conditions {
                    Self::collect_aggregate_args_recursive(when_expr, args_out);
                    Self::collect_aggregate_args_recursive(then_expr, args_out);
                }
                if let Some(def) = default {
                    Self::collect_aggregate_args_recursive(def, args_out);
                }
            }
            Expression::Property { object, .. } => {
                Self::collect_aggregate_args_recursive(object, args_out);
            }
            Expression::StructField { base, .. } => {
                Self::collect_aggregate_args_recursive(base, args_out);
            }
            Expression::Subscript { collection, index } => {
                Self::collect_aggregate_args_recursive(collection, args_out);
                Self::collect_aggregate_args_recursive(index, args_out);
            }
            Expression::Range {
                collection,
                start,
                end,
            } => {
                Self::collect_aggregate_args_recursive(collection, args_out);
                if let Some(s) = start {
                    Self::collect_aggregate_args_recursive(s, args_out);
                }
                if let Some(e) = end {
                    Self::collect_aggregate_args_recursive(e, args_out);
                }
            }
            Expression::Path(items) => {
                for item in items {
                    Self::collect_aggregate_args_recursive(item, args_out);
                }
            }
            Expression::TypeCast { expression, .. } => {
                Self::collect_aggregate_args_recursive(expression, args_out);
            }
            Expression::ListComprehension {
                source,
                filter,
                map,
                ..
            } => {
                Self::collect_aggregate_args_recursive(source, args_out);
                if let Some(f) = filter {
                    Self::collect_aggregate_args_recursive(f, args_out);
                }
                if let Some(m) = map {
                    Self::collect_aggregate_args_recursive(m, args_out);
                }
            }
            Expression::LabelTagProperty { tag, .. } => {
                Self::collect_aggregate_args_recursive(tag, args_out);
            }
            Expression::Predicate { args, .. } => {
                for arg in args {
                    Self::collect_aggregate_args_recursive(arg, args_out);
                }
            }
            Expression::Reduce {
                initial,
                source,
                mapping,
                ..
            } => {
                Self::collect_aggregate_args_recursive(initial, args_out);
                Self::collect_aggregate_args_recursive(source, args_out);
                Self::collect_aggregate_args_recursive(mapping, args_out);
            }
            Expression::PathBuild(items) => {
                for item in items {
                    Self::collect_aggregate_args_recursive(item, args_out);
                }
            }
            Expression::Literal(_)
            | Expression::Variable(_)
            | Expression::Label(_)
            | Expression::TagProperty { .. }
            | Expression::EdgeProperty { .. }
            | Expression::Parameter(_)
            | Expression::SessionVariable(_)
            | Expression::Vector(_)
            | Expression::Exists { .. }
            | Expression::In { .. }
            | Expression::WindowFunction { .. } => {}
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
        input: Some(Box::new(input.clone())),
        deps: vec![input],
        group_key_exprs,
        aggregation_functions: aggregate_node.aggregation_functions().to_vec(),
        aggregation_distinct: aggregate_node.aggregation_distinct().to_vec(),
        aggregation_filters: aggregate_node.aggregation_filters().to_vec(),
        grouping_sets: aggregate_node.grouping_sets().to_vec(),
        output_var: None,
        col_names: aggregate_node.col_names().to_vec(),
        column_types: vec![],
    })
}
