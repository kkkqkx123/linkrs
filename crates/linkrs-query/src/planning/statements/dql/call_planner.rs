//! `CALL <function>(...)` planner.
//!
//! Plans an in-query function call into a physical-only leaf node. Arguments
//! are evaluated at plan time and passed to the executor as a JSON value
//! array; `YIELD` is a column-name list whose aliases rename the produced
//! columns.

use crate::binder::BoundStatement;
use crate::parser::ast::{InQueryCallStmt, Stmt, YieldClause};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::management::InQueryCallNode;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use crate::planning::statements::dql::argument_literals::literal_arguments_json;

/// Planner for `CALL`.
#[derive(Debug, Clone, Default)]
pub struct InQueryCallPlanner;

impl InQueryCallPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl Planner for InQueryCallPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let call = match ctx.bound {
            BoundStatement::Other(stmt) => match stmt.as_ref() {
                Stmt::InQueryCall(call) => call,
                _ => {
                    return Err(PlannerError::PlanGenerationFailed(
                        "statement does not contain CALL".to_string(),
                    ))
                }
            },
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain CALL".to_string(),
                ))
            }
        };

        let args_json = literal_arguments_json(&call.args, "CALL")?;
        let yield_items = Self::yield_items(call)?;
        let col_names = yield_items
            .iter()
            .map(|(column, _)| column.clone())
            .collect();

        let node = InQueryCallNode::new(
            next_node_id(),
            call.func_name.clone(),
            args_json,
            yield_items,
            col_names,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }
}

impl InQueryCallPlanner {
    /// Convert `YIELD a [AS b], ...` into `(output column, source column)`.
    ///
    /// The parser accepts identifiers only, so a non-variable expression here
    /// means the AST was built without the parser; reject it rather than
    /// silently dropping the projection.
    fn yield_items(call: &InQueryCallStmt) -> Result<Vec<(String, String)>, PlannerError> {
        let Some(clause) = &call.yield_clause else {
            return Ok(Vec::new());
        };
        Self::convert_yield_clause(clause)
    }

    fn convert_yield_clause(clause: &YieldClause) -> Result<Vec<(String, String)>, PlannerError> {
        clause
            .items
            .iter()
            .map(|item| {
                let source = item.expression.as_variable().ok_or_else(|| {
                    PlannerError::UnsupportedOperation(
                        "CALL YIELD accepts column names only".to_string(),
                    )
                })?;
                let column = item.alias.clone().unwrap_or_else(|| source.clone());
                Ok((column, source))
            })
            .collect()
    }
}
