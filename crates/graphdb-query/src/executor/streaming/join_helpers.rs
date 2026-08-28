use std::sync::Arc;

use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::executor::ValueRowContext;
use crate::executor::streaming::slot::{combine_layouts, SlotLayout};
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::Value;

/// Evaluate join key expressions against a row, returning the key values.
pub fn evaluate_join_key(
    row: &[Value],
    layout: Arc<SlotLayout>,
    key_expressions: &[Expression],
) -> Result<Vec<Value>, QueryError> {
    if key_expressions.is_empty() {
        return Ok(Vec::new());
    }
    let mut key = Vec::with_capacity(key_expressions.len());
    for expr in key_expressions {
        let mut context = ValueRowContext::new(row.to_vec(), layout.clone());
        let value = ExpressionEvaluator::evaluate(expr, &mut context)
            .map_err(|e| QueryError::execution(format!("HashJoin key evaluation failed: {e}")))?;
        key.push(value);
    }
    Ok(key)
}

/// Evaluate a residual join condition given left and right rows with their schemas.
pub fn evaluate_residual_condition(
    condition: &Expression,
    left_row: &[Value],
    right_row: &[Value],
    left_schema: &[String],
    right_schema: &[String],
) -> Result<bool, QueryError> {
    let mut combined = left_row.to_vec();
    combined.extend_from_slice(right_row);
    let combined_layout = build_combined_layout_from_schemas(left_schema, right_schema);
    let mut context = ValueRowContext::new(combined, combined_layout);
    match ExpressionEvaluator::evaluate(condition, &mut context) {
        Ok(Value::Bool(b)) => Ok(b),
        Ok(Value::Null(_)) => Ok(false),
        Ok(_) => Ok(true),
        Err(e) => Err(QueryError::execution(format!(
            "HashJoin condition evaluation failed: {e}"
        ))),
    }
}

/// Build a combined slot layout from left and right column name schemas.
pub fn build_combined_layout_from_schemas(
    left_schema: &[String],
    right_schema: &[String],
) -> Arc<SlotLayout> {
    let left_layout = Arc::new(SlotLayout::from_names(left_schema));
    let right_layout = Arc::new(SlotLayout::from_names(right_schema));
    Arc::new(combine_layouts(&left_layout, &right_layout))
}
