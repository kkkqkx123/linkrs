//! Standalone COLLECT Statement Planner
//!
//! A standalone COLLECT stage aggregates all input rows into a single output
//! row, e.g. `GO FROM 1 OVER KNOWS | YIELD target.name AS name
//! | COLLECT LIST(name) AS names`.

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::Stmt;
use crate::planning::plan::core::nodes::{AggregateNode, StartNode};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use graphdb_core::types::operators::AggregateFunction;
use std::sync::Arc;

/// Standalone COLLECT statement planner.
#[derive(Debug, Clone)]
pub struct CollectPlanner;

impl CollectPlanner {
    pub fn new() -> Self {
        Self
    }

    /// Extract the input field referenced by a COLLECT item from a BoundExpression.
    fn collect_bound_field(&self, expr: &crate::binder::bound::BoundExpression) -> String {
        use crate::binder::bound::BoundExpression;
        if let BoundExpression::Function(f) = expr {
            if f.name.eq_ignore_ascii_case("list") && f.args.len() == 1 {
                return Self::bound_expr_to_variable_string(&f.args[0]);
            }
        }
        Self::bound_expr_to_variable_string(expr)
    }

    fn bound_expr_to_variable_string(expr: &crate::binder::bound::BoundExpression) -> String {
        use crate::binder::bound::BoundExpression;
        match expr {
            BoundExpression::Variable(name, _) => name.clone(),
            BoundExpression::ColumnRef(cr) => format!("{}.{}", cr.variable, cr.property),
            _ => "_".to_string(),
        }
    }
}

impl Planner for CollectPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let collect_stmt = match validated.stmt() {
            Stmt::Collect(collect_stmt) => collect_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "CollectPlanner requires the Collect statement.".to_string(),
                ));
            }
        };

        let start_node = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start_node);

        let mut aggregate_functions = Vec::new();
        let mut aggregation_args = Vec::new();
        let mut agg_aliases = Vec::new();
        for item in &collect_stmt.items {
            let expression = item
                .expression
                .expression()
                .map(|e| e.inner().clone())
                .unwrap_or_else(|| {
                    graphdb_core::types::Expression::Variable(
                        item.expression.to_expression_string(),
                    )
                });
            let field = match &expression {
                graphdb_core::types::Expression::Function { name, args }
                    if name.eq_ignore_ascii_case("list") && args.len() == 1 =>
                {
                    args[0].to_expression_string()
                }
                other => other.to_expression_string(),
            };
            aggregate_functions.push(AggregateFunction::Collect);
            aggregation_args.push(vec![graphdb_core::types::Expression::Variable(field)]);
            agg_aliases.push(
                item.alias
                    .clone()
                    .unwrap_or_else(|| "collected".to_string()),
            );
        }

        let mut aggregate_node = AggregateNode::with_agg_aliases(
            start_enum.clone(),
            Vec::new(),
            aggregate_functions,
            agg_aliases,
        )
        .map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create AggregateNode: {}", e))
        })?;
        aggregate_node.set_aggregation_args(aggregation_args);

        let sub_plan = SubPlan::new(
            Some(PlanNodeEnum::Aggregate(aggregate_node)),
            Some(start_enum),
        );
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
        let collect = match bound {
            BoundStatement::Collect(c) => c,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "CollectPlanner requires BoundStatement::Collect.".to_string(),
                ));
            }
        };

        let start_node = StartNode::new();
        let start_enum = PlanNodeEnum::Start(start_node);

        let mut aggregate_functions = Vec::new();
        let mut aggregation_args = Vec::new();
        let mut agg_aliases = Vec::new();
        for item in &collect.items {
            let field = self.collect_bound_field(&item.expression);
            aggregate_functions.push(AggregateFunction::Collect);
            aggregation_args.push(vec![graphdb_core::types::Expression::Variable(field)]);
            agg_aliases.push(
                item.alias
                    .clone()
                    .unwrap_or_else(|| "collected".to_string()),
            );
        }

        let mut aggregate_node = AggregateNode::with_agg_aliases(
            start_enum.clone(),
            Vec::new(),
            aggregate_functions,
            agg_aliases,
        )
        .map_err(|e| {
            PlannerError::PlanGenerationFailed(format!("Failed to create AggregateNode: {}", e))
        })?;
        aggregate_node.set_aggregation_args(aggregation_args);

        let sub_plan = SubPlan::new(
            Some(PlanNodeEnum::Aggregate(aggregate_node)),
            Some(start_enum),
        );
        Ok(sub_plan)
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(stmt, Stmt::Collect(_))
    }
}

impl Default for CollectPlanner {
    fn default() -> Self {
        Self::new()
    }
}
