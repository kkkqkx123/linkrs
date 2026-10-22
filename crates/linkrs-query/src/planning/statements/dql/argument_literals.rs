//! Plan-time evaluation of literal arguments.
//!
//! `LOAD FROM` table functions and `CALL` pass their arguments to the
//! executor as a JSON value array. Only literals are accepted: evaluating
//! arbitrary expressions there would require an expression evaluator inside
//! the DDL operator, which is outside its responsibility.

use crate::planning::planner::PlannerError;
use linkrs_core::types::expr::contextual::ContextualExpression;
use linkrs_core::Value;

/// Serialize `args` as a JSON array of literal values.
///
/// `clause` names the statement in error messages.
pub(super) fn literal_arguments_json(
    args: &[ContextualExpression],
    clause: &str,
) -> Result<String, PlannerError> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        let value = arg.as_literal().ok_or_else(|| {
            PlannerError::UnsupportedOperation(format!(
                "{} arguments must be literals, got: {}",
                clause,
                arg.to_expression_string()
            ))
        })?;
        values.push(value_to_json(&value, clause)?);
    }
    serde_json::to_string(&values).map_err(|e| {
        PlannerError::PlanGenerationFailed(format!("{} arguments cannot be encoded: {}", clause, e))
    })
}

fn value_to_json(value: &Value, clause: &str) -> Result<serde_json::Value, PlannerError> {
    match value {
        Value::Empty | Value::Null(_) => Ok(serde_json::Value::Null),
        Value::Bool(b) => Ok(serde_json::Value::Bool(*b)),
        Value::SmallInt(i) => Ok(serde_json::Value::from(*i as i64)),
        Value::Int(i) => Ok(serde_json::Value::from(*i as i64)),
        Value::BigInt(i) => Ok(serde_json::Value::from(*i)),
        Value::Float(f) => number_from_f64(*f as f64, clause),
        Value::Double(f) => number_from_f64(*f, clause),
        Value::String(s) => Ok(serde_json::Value::String(s.to_string())),
        Value::FixedString(s) => Ok(serde_json::Value::String(s.clone())),
        Value::List(list) => list
            .iter()
            .map(|item| value_to_json(item, clause))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        Value::Map(map) => {
            let mut object = serde_json::Map::with_capacity(map.len());
            for (key, item) in map.iter() {
                let key = match key {
                    Value::String(s) => s.to_string(),
                    Value::FixedString(s) => s.clone(),
                    other => {
                        return Err(PlannerError::UnsupportedOperation(format!(
                            "{} map arguments require string keys, got: {}",
                            clause, other
                        )))
                    }
                };
                object.insert(key, value_to_json(item, clause)?);
            }
            Ok(serde_json::Value::Object(object))
        }
        other => Err(PlannerError::UnsupportedOperation(format!(
            "{} argument type is not supported: {}",
            clause, other
        ))),
    }
}

fn number_from_f64(value: f64, clause: &str) -> Result<serde_json::Value, PlannerError> {
    serde_json::Number::from_f64(value)
        .map(serde_json::Value::Number)
        .ok_or_else(|| {
            PlannerError::UnsupportedOperation(format!(
                "{} arguments cannot carry non-finite numbers: {}",
                clause, value
            ))
        })
}
