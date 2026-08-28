//! Standalone COLLECT Statement Planner
//!
//! A standalone COLLECT stage aggregates all input rows into a single output
//! row, e.g. `GO FROM 1 OVER KNOWS | YIELD target.name AS name
//! | COLLECT LIST(name) AS names`.

use crate::parser::ast::stmt::{CollectStmt, Stmt};
use crate::planning::plan::core::nodes::{AggregateNode, StartNode};
use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::QueryContext;
use graphdb_core::types::expr::Expression;
use graphdb_core::types::operators::AggregateFunction;
use std::sync::Arc;

/// Standalone COLLECT statement planner.
#[derive(Debug, Clone)]
pub struct CollectPlanner;

impl CollectPlanner {
    pub fn new() -> Self {
        Self
    }

    /// Extract the input field referenced by a COLLECT item.
    ///
    /// `LIST(name)` collects the `name` column of every input row into a list.
    fn collect_field(&self, expression: &Expression) -> String {
        if let Expression::Function { name, args } = expression {
            if name.eq_ignore_ascii_case("list") && args.len() == 1 {
                return args[0].to_expression_string();
            }
        }
        expression.to_expression_string()
    }
}

impl Planner for CollectPlanner {
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        _qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        let collect_stmt: &CollectStmt = match validated.stmt() {
            Stmt::Collect(collect_stmt) => collect_stmt,
            _ => {
                return Err(PlannerError::InvalidOperation(
                    "CollectPlanner requires the Collect statement.".to_string(),
                ));
            }
        };

        // A single empty row seeds a standalone COLLECT stage. When the stage
        // is the right side of a pipe, PipePlanner replaces it with the piped
        // rows, and the aggregate collapses them into one row.
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
                .unwrap_or_else(|| Expression::Variable(item.expression.to_expression_string()));
            let field = self.collect_field(&expression);
            aggregate_functions.push(AggregateFunction::Collect);
            aggregation_args.push(vec![Expression::Variable(field)]);
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
