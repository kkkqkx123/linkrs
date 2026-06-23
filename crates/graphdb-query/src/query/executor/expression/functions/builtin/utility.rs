//! Implementation of practical functions

use crate::core::value::list::List;
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;
use serde_json::Value as JsonValue;

/// Enumeration of practical functions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UtilityFunction {
    Coalesce,
    Hash,
    JsonExtract,
}

impl UtilityFunction {
    pub fn name(&self) -> &str {
        match self {
            UtilityFunction::Coalesce => "coalesce",
            UtilityFunction::Hash => "hash",
            UtilityFunction::JsonExtract => "json_extract",
        }
    }

    pub fn arity(&self) -> usize {
        match self {
            UtilityFunction::Coalesce => 1,
            UtilityFunction::Hash => 1,
            UtilityFunction::JsonExtract => 2,
        }
    }

    pub fn is_variadic(&self) -> bool {
        matches!(self, UtilityFunction::Coalesce)
    }

    pub fn description(&self) -> &str {
        match self {
            UtilityFunction::Coalesce => "Returns the first non-NULL value",
            UtilityFunction::Hash => "Compute the hash",
            UtilityFunction::JsonExtract => {
                "Extract the value of a specified path from a JSON string"
            }
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            UtilityFunction::Coalesce => execute_coalesce(args),
            UtilityFunction::Hash => execute_hash(args),
            UtilityFunction::JsonExtract => execute_json_extract(args),
        }
    }
}

fn execute_coalesce(args: &[Value]) -> Result<Value, ExpressionError> {
    for arg in args {
        match arg {
            Value::Null(_) => continue,
            other => return Ok(other.clone()),
        }
    }
    Ok(Value::Null(NullType::Null))
}

fn execute_hash(args: &[Value]) -> Result<Value, ExpressionError> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    match &args[0] {
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        Value::String(s) => {
            let mut hasher = DefaultHasher::new();
            s.hash(&mut hasher);
            let hash_value = hasher.finish() as i64;
            Ok(Value::BigInt(hash_value))
        }
        Value::Int(i) => {
            let mut hasher = DefaultHasher::new();
            i.hash(&mut hasher);
            let hash_value = hasher.finish() as i64;
            Ok(Value::BigInt(hash_value))
        }
        _ => Err(ExpressionError::type_error(
            "The hash function requires a string or integer type",
        )),
    }
}

fn execute_json_extract(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::String(json_str), Value::String(path)) => {
            let json_value: JsonValue = serde_json::from_str(json_str)
                .map_err(|_| ExpressionError::type_error("Invalid JSON string"))?;

            let result = extract_json_value(&json_value, path);
            Ok(json_to_value(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The json_extract function takes string arguments",
        )),
    }
}

fn extract_json_value<'a>(json: &'a JsonValue, path: &str) -> &'a JsonValue {
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = json;

    for part in parts {
        if part.is_empty() {
            continue;
        }

        current = match current {
            JsonValue::Object(map) => map.get(part).unwrap_or(&JsonValue::Null),
            JsonValue::Array(arr) => {
                if let Ok(index) = part.parse::<usize>() {
                    arr.get(index).unwrap_or(&JsonValue::Null)
                } else {
                    &JsonValue::Null
                }
            }
            _ => &JsonValue::Null,
        };
    }

    current
}

fn json_to_value(json: &JsonValue) -> Value {
    match json {
        JsonValue::Null => Value::Null(NullType::Null),
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::BigInt(i)
            } else if let Some(f) = n.as_f64() {
                Value::Double(f)
            } else {
                Value::Null(NullType::Null)
            }
        }
        JsonValue::String(s) => Value::String(s.clone()),
        JsonValue::Array(arr) => {
            let values: Vec<Value> = arr.iter().map(json_to_value).collect();
            Value::list(List { values })
        }
        JsonValue::Object(obj) => {
            let map: std::collections::HashMap<String, Value> = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect();
            Value::map(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_coalesce() {
        let func = UtilityFunction::Coalesce;
        let result = func
            .execute(&[Value::Null(NullType::Null), Value::Int(42), Value::Int(100)])
            .expect("Execution should succeed");
        assert_eq!(result, Value::Int(42));
    }

    #[test]
    fn test_hash() {
        let func = UtilityFunction::Hash;
        let result = func
            .execute(&[Value::String("test".to_string())])
            .expect("Execution should succeed");
        assert!(matches!(result, Value::BigInt(_)));
    }
}
