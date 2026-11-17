use linkrs_core::types::expr::Expression;
use linkrs_core::value::NullType;
use linkrs_core::Value;

use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::executor::ValueRowContext;

/// Evaluate one aggregate `FILTER (WHERE ...)` predicate for a row.
///
/// `None` aggregates every row. A non-true outcome (false, NULL, or an
/// evaluation error) excludes the row from that aggregate only, mirroring
/// SQL FILTER semantics where only true qualifies.
pub(crate) fn eval_aggregate_filter(
    filter: &Option<Expression>,
    row: &[Value],
    col_names: &[String],
) -> bool {
    match filter {
        None => true,
        Some(expr) => {
            let mut ctx = ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
            matches!(
                ExpressionEvaluator::evaluate(expr, &mut ctx),
                Ok(Value::Bool(true))
            )
        }
    }
}

/// Evaluate the `GROUP BY` key of one row.
///
/// A key component that cannot be evaluated for this row becomes NULL, which
/// is SQL's own propagation rule for an unknown key value: the row joins the
/// NULL group instead of aborting the whole aggregation. Batch evaluation is
/// the fast path; this is the authoritative per-row fallback, so all
/// aggregation phases must funnel through it to keep group identity equal.
pub(crate) fn eval_group_key(
    row: &[Value],
    col_names: &[String],
    group_by_expressions: &[Expression],
) -> Vec<Value> {
    group_by_expressions
        .iter()
        .map(|expr| {
            let mut ctx = ValueRowContext::from_names(row.to_vec(), col_names.to_vec());
            ExpressionEvaluator::evaluate(expr, &mut ctx).unwrap_or(Value::Null(NullType::Null))
        })
        .collect()
}
