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
    JsonBuildObject,
    JsonBuildArray,
    JsonObjectKeys,
    NullIf,
    Greatest,
    Least,
    GenRandomUuid,
    JsonEach,
    JsonTypeOf,
    JsonStripNulls,
}

impl UtilityFunction {
    pub fn name(&self) -> &str {
        match self {
            UtilityFunction::Coalesce => "coalesce",
            UtilityFunction::Hash => "hash",
            UtilityFunction::JsonExtract => "json_extract",
            UtilityFunction::JsonBuildObject => "json_build_object",
            UtilityFunction::JsonBuildArray => "json_build_array",
            UtilityFunction::JsonObjectKeys => "json_object_keys",
            UtilityFunction::NullIf => "nullif",
            UtilityFunction::Greatest => "greatest",
            UtilityFunction::Least => "least",
            UtilityFunction::GenRandomUuid => "gen_random_uuid",
            UtilityFunction::JsonEach => "json_each",
            UtilityFunction::JsonTypeOf => "json_typeof",
            UtilityFunction::JsonStripNulls => "json_strip_nulls",
        }
    }

    pub fn arity(&self) -> usize {
        match self {
            UtilityFunction::Coalesce => 1,
            UtilityFunction::Hash => 1,
            UtilityFunction::JsonExtract => 2,
            UtilityFunction::JsonBuildObject => 0,
            UtilityFunction::JsonBuildArray => 0,
            UtilityFunction::JsonObjectKeys => 1,
            UtilityFunction::NullIf => 2,
            UtilityFunction::Greatest => 2,
            UtilityFunction::Least => 2,
            UtilityFunction::GenRandomUuid => 0,
            UtilityFunction::JsonEach => 1,
            UtilityFunction::JsonTypeOf => 1,
            UtilityFunction::JsonStripNulls => 1,
        }
    }

    pub fn is_variadic(&self) -> bool {
        matches!(
            self,
            UtilityFunction::Coalesce
                | UtilityFunction::Greatest
                | UtilityFunction::Least
                | UtilityFunction::JsonBuildObject
                | UtilityFunction::JsonBuildArray
        )
    }

    pub fn description(&self) -> &str {
        match self {
            UtilityFunction::Coalesce => "Returns the first non-NULL value",
            UtilityFunction::Hash => "Compute the hash",
            UtilityFunction::JsonExtract => {
                "Extract the value of a specified path from a JSON string"
            }
            UtilityFunction::JsonBuildObject => "Build a JSON object from key-value pairs",
            UtilityFunction::JsonBuildArray => "Build a JSON array from a list of values",
            UtilityFunction::JsonObjectKeys => "Get keys from a JSON object",
            UtilityFunction::NullIf => "Return NULL if two values are equal",
            UtilityFunction::Greatest => "Return the largest of the arguments",
            UtilityFunction::Least => "Return the smallest of the arguments",
            UtilityFunction::GenRandomUuid => "Generate a random UUID v4",
            UtilityFunction::JsonEach => "Expand a JSON object into key-value pairs",
            UtilityFunction::JsonTypeOf => "Return the type of a JSON value",
            UtilityFunction::JsonStripNulls => "Strip null values from a JSON object or array",
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            UtilityFunction::Coalesce => execute_coalesce(args),
            UtilityFunction::Hash => execute_hash(args),
            UtilityFunction::JsonExtract => execute_json_extract(args),
            UtilityFunction::JsonBuildObject => execute_json_build_object(args),
            UtilityFunction::JsonBuildArray => execute_json_build_array(args),
            UtilityFunction::JsonObjectKeys => execute_json_object_keys(args),
            UtilityFunction::NullIf => execute_nullif(args),
            UtilityFunction::Greatest => execute_greatest(args),
            UtilityFunction::Least => execute_least(args),
            UtilityFunction::GenRandomUuid => execute_gen_random_uuid(args),
            UtilityFunction::JsonEach => execute_json_each(args),
            UtilityFunction::JsonTypeOf => execute_json_typeof(args),
            UtilityFunction::JsonStripNulls => execute_json_strip_nulls(args),
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

fn execute_json_build_object(args: &[Value]) -> Result<Value, ExpressionError> {
    use serde_json::Map;

    if args.len() % 2 != 0 {
        return Err(ExpressionError::type_error(
            "json_build_object requires an even number of arguments (key-value pairs)",
        ));
    }

    let mut map = Map::new();
    for chunk in args.chunks(2) {
        let key = match &chunk[0] {
            Value::String(s) => s.clone(),
            Value::Null(_) => continue,
            _ => {
                return Err(ExpressionError::type_error(
                    "json_build_object keys must be strings",
                ))
            }
        };
        let value = value_to_json_value(&chunk[1]);
        map.insert(key, value);
    }

    Ok(Value::String(
        serde_json::to_string(&JsonValue::Object(map))
            .map_err(|e| ExpressionError::type_error(format!("JSON serialization error: {}", e)))?,
    ))
}

fn execute_json_build_array(args: &[Value]) -> Result<Value, ExpressionError> {
    let arr: Vec<JsonValue> = args.iter().map(value_to_json_value).collect();
    Ok(Value::String(
        serde_json::to_string(&JsonValue::Array(arr))
            .map_err(|e| ExpressionError::type_error(format!("JSON serialization error: {}", e)))?,
    ))
}

fn execute_json_object_keys(args: &[Value]) -> Result<Value, ExpressionError> {
    use crate::core::value::list::List;

    match &args[0] {
        Value::String(json_str) => {
            let json_value: JsonValue = serde_json::from_str(json_str)
                .map_err(|_| ExpressionError::type_error("Invalid JSON string"))?;

            match &json_value {
                JsonValue::Object(map) => {
                    let keys: Vec<Value> = map
                        .keys()
                        .map(|k| Value::String(k.clone()))
                        .collect();
                    Ok(Value::list(List { values: keys }))
                }
                _ => Err(ExpressionError::type_error(
                    "json_object_keys requires a JSON object",
                )),
            }
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "json_object_keys requires a string argument",
        )),
    }
}

fn value_to_json_value(value: &Value) -> JsonValue {
    match value {
        Value::Null(_) => JsonValue::Null,
        Value::Bool(b) => JsonValue::Bool(*b),
        Value::SmallInt(i) => JsonValue::Number((*i).into()),
        Value::Int(i) => JsonValue::Number((*i).into()),
        Value::BigInt(i) => JsonValue::Number((*i).into()),
        Value::Float(f) => JsonValue::Number(
            serde_json::Number::from_f64(*f as f64).unwrap_or(serde_json::Number::from(0)),
        ),
        Value::Double(f) => JsonValue::Number(
            serde_json::Number::from_f64(*f).unwrap_or(serde_json::Number::from(0)),
        ),
        Value::String(s) => JsonValue::String(s.clone()),
        Value::List(list) => {
            JsonValue::Array(list.values.iter().map(value_to_json_value).collect())
        }
        Value::Map(map) => {
            let obj: serde_json::Map<String, JsonValue> = map
                .iter()
                .map(|(k, v)| (k.clone(), value_to_json_value(v)))
                .collect();
            JsonValue::Object(obj)
        }
        _ => JsonValue::Null,
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

fn execute_nullif(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.len() != 2 {
        return Err(ExpressionError::type_error(
            "nullif requires 2 arguments",
        ));
    }
    match (&args[0], &args[1]) {
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        (a, b) => {
            if values_equal(a, b) {
                Ok(Value::Null(NullType::Null))
            } else {
                Ok(a.clone())
            }
        }
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(a), Value::BigInt(b)) => *a as i64 == *b,
        (Value::BigInt(a), Value::Int(b)) => *a == *b as i64,
        (Value::Float(a), Value::Double(b)) => *a as f64 == *b,
        (Value::Double(a), Value::Float(b)) => *a == *b as f64,
        _ => a == b,
    }
}

fn values_less_than(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(a), Value::Int(b)) => a < b,
        (Value::BigInt(a), Value::BigInt(b)) => a < b,
        (Value::Float(a), Value::Float(b)) => a < b,
        (Value::Double(a), Value::Double(b)) => a < b,
        (Value::String(a), Value::String(b)) => a < b,
        (Value::Int(a), Value::BigInt(b)) => (*a as i64) < *b,
        (Value::BigInt(a), Value::Int(b)) => *a < (*b as i64),
        (Value::Float(a), Value::Double(b)) => (*a as f64) < *b,
        (Value::Double(a), Value::Float(b)) => *a < (*b as f64),
        _ => false,
    }
}

fn execute_greatest(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.is_empty() {
        return Err(ExpressionError::type_error(
            "greatest requires at least 1 argument",
        ));
    }
    let mut result = &args[0];
    for arg in &args[1..] {
        if values_less_than(result, arg) {
            result = arg;
        }
    }
    Ok(result.clone())
}

fn execute_least(args: &[Value]) -> Result<Value, ExpressionError> {
    if args.is_empty() {
        return Err(ExpressionError::type_error(
            "least requires at least 1 argument",
        ));
    }
    let mut result = &args[0];
    for arg in &args[1..] {
        if values_less_than(arg, result) {
            result = arg;
        }
    }
    Ok(result.clone())
}

fn execute_gen_random_uuid(_args: &[Value]) -> Result<Value, ExpressionError> {
    use crate::core::value::uuid::UuidValue;
    Ok(Value::Uuid(UuidValue::new_v4()))
}

fn execute_json_each(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(json_str) => {
            let json_value: JsonValue = serde_json::from_str(json_str)
                .map_err(|_| ExpressionError::type_error("Invalid JSON string"))?;
            match json_value {
                JsonValue::Object(map) => {
                    let entries: Vec<Value> = map
                        .into_iter()
                        .map(|(k, v)| {
                            Value::list(List {
                                values: vec![Value::String(k), json_to_value(&v)],
                            })
                        })
                        .collect();
                    Ok(Value::list(List { values: entries }))
                }
                JsonValue::Array(arr) => {
                    let entries: Vec<Value> = arr
                        .into_iter()
                        .enumerate()
                        .map(|(i, v)| {
                            Value::list(List {
                                values: vec![Value::BigInt(i as i64), json_to_value(&v)],
                            })
                        })
                        .collect();
                    Ok(Value::list(List { values: entries }))
                }
                _ => Err(ExpressionError::type_error(
                    "json_each requires a JSON object or array",
                )),
            }
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "json_each requires a string argument",
        )),
    }
}

fn execute_json_typeof(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(json_str) => {
            let json_value: JsonValue = serde_json::from_str(json_str)
                .map_err(|_| ExpressionError::type_error("Invalid JSON string"))?;
            let type_str = match &json_value {
                JsonValue::Null => "null",
                JsonValue::Bool(_) => "boolean",
                JsonValue::Number(_) => "number",
                JsonValue::String(_) => "string",
                JsonValue::Array(_) => "array",
                JsonValue::Object(_) => "object",
            };
            Ok(Value::String(type_str.to_string()))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "json_typeof requires a string argument",
        )),
    }
}

fn execute_json_strip_nulls(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(json_str) => {
            let json_value: JsonValue = serde_json::from_str(json_str)
                .map_err(|_| ExpressionError::type_error("Invalid JSON string"))?;
            let stripped = strip_nulls_from_json(json_value);
            Ok(Value::String(
                serde_json::to_string(&stripped)
                    .map_err(|e| ExpressionError::type_error(format!("JSON serialization error: {}", e)))?,
            ))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "json_strip_nulls requires a string argument",
        )),
    }
}

fn strip_nulls_from_json(value: JsonValue) -> JsonValue {
    match value {
        JsonValue::Object(map) => {
            let cleaned: serde_json::Map<String, JsonValue> = map
                .into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls_from_json(v)))
                .collect();
            JsonValue::Object(cleaned)
        }
        JsonValue::Array(arr) => {
            let cleaned: Vec<JsonValue> = arr
                .into_iter()
                .filter(|v| !v.is_null())
                .map(strip_nulls_from_json)
                .collect();
            JsonValue::Array(cleaned)
        }
        other => other,
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
