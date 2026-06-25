//! GroupBy Operation Planner
//!
//! Query planning for statements that involve the GROUP BY clause

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::expr::Expression;
use crate::core::types::operators::AggregateFunction;
use crate::query::parser::ast::{GroupingType, Stmt};
use crate::query::planning::plan::core::{
    node_id_generator::next_node_id,
    nodes::{AggregateNode, ArgumentNode, FilterNode},
};
use crate::query::planning::plan::{PlanNodeEnum, SubPlan};
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;
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
                functions.push((func.clone(), *distinct, filter.as_ref().map(|f| f.as_ref().clone())));
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
            | Expression::Vector(_) => {}
        }
    }
}

impl Planner for GroupByPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let group_by_stmt = match validated.stmt() {
            Stmt::GroupBy(group_by_stmt) => group_by_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "GroupByPlanner requires the GroupBy statement.".to_string(),
                ));
            }
        };

        // Create a parameter node as the input.
        let arg_node = ArgumentNode::new(next_node_id(), "group_by_input");
        let arg_node_enum = PlanNodeEnum::Argument(arg_node.clone());

        // Extract the group key – Use an expression to describe the key.
        let num_group_items = group_by_stmt.group_items.len();
        let group_keys: Vec<String> = group_by_stmt
            .group_items
            .iter()
            .enumerate()
            .map(|(i, _)| format!("group_key_{}", i))
            .collect();

        // Extract the aggregate functions with distinct flags and filters
        let mut aggregation_functions = Vec::new();
        let mut aggregation_distinct = Vec::new();
        let mut aggregation_filters = Vec::new();
        for item in &group_by_stmt.yield_clause.items {
            let funcs = self.extract_aggregate_functions(&item.expression);
            for (func, distinct, filter) in funcs {
                aggregation_functions.push(func);
                aggregation_distinct.push(distinct);
                aggregation_filters.push(filter);
            }
        }

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
                    for i in 0..num_group_items {
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
                sets.sort_by(|a, b| b.len().cmp(&a.len()));
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
                                        gi.expression()
                                            .map(|m| m.inner().to_string())
                                            == expr_str
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
            arg_node_enum.clone(),
            group_keys,
            aggregation_functions,
        )
        .map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create AggregateNode: {}", e))
        })?;
        aggregate_node.set_aggregation_distinct(aggregation_distinct);
        aggregate_node.set_aggregation_filters(aggregation_filters);
        aggregate_node.set_grouping_sets(grouping_sets);

        let mut final_node = PlanNodeEnum::Aggregate(aggregate_node);

        // If there is a HAVING clause, add a FilterNode.
        if let Some(ref having_expr) = group_by_stmt.having_clause {
            let filter_node =
                FilterNode::new(final_node.clone(), having_expr.clone()).map_err(|e| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Failed to create FilterNode: {}",
                        e
                    ))
                })?;
            final_node = PlanNodeEnum::Filter(filter_node);
        }

        // Create a SubPlan
        let sub_plan = SubPlan::new(Some(final_node), Some(arg_node_enum));

        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::GroupBy(_))
    }
}

impl Default for GroupByPlanner {
    fn default() -> Self {
        Self::new()
    }
}
