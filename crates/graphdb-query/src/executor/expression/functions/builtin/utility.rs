//! Implementation of practical functions

use crate::executor::expression::ExpressionError;
use graphdb_core::value::list::List;
use graphdb_core::value::NullType;
use graphdb_core::Value;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

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
    IfNull,
    TypeOf,
    Version,
    CurrentUser,
    CurrentDatabase,
    Corr,
    CovarPop,
    CovarSamp,
    OctetLength,
    Encode,
    Decode,
    UnionValue,
    UnionTag,
    UnionExtract,
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
            UtilityFunction::IfNull => "ifnull",
            UtilityFunction::TypeOf => "typeof",
            UtilityFunction::Version => "version",
            UtilityFunction::CurrentUser => "current_user",
            UtilityFunction::CurrentDatabase => "current_database",
            UtilityFunction::Corr => "corr",
            UtilityFunction::CovarPop => "covar_pop",
            UtilityFunction::CovarSamp => "covar_samp",
            UtilityFunction::OctetLength => "octet_length",
            UtilityFunction::Encode => "encode",
            UtilityFunction::Decode => "decode",
            UtilityFunction::UnionValue => "union_value",
            UtilityFunction::UnionTag => "union_tag",
            UtilityFunction::UnionExtract => "union_extract",
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
            UtilityFunction::IfNull => 2,
            UtilityFunction::TypeOf => 1,
            UtilityFunction::Version => 0,
            UtilityFunction::CurrentUser => 0,
            UtilityFunction::CurrentDatabase => 0,
            UtilityFunction::Corr => 2,
            UtilityFunction::CovarPop => 2,
            UtilityFunction::CovarSamp => 2,
            UtilityFunction::OctetLength => 1,
            UtilityFunction::Encode => 1,
            UtilityFunction::Decode => 1,
            UtilityFunction::UnionValue => 2,
            UtilityFunction::UnionTag => 1,
            UtilityFunction::UnionExtract => 1,
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
            UtilityFunction::IfNull => {
                "Return first argument if not NULL, otherwise second argument"
            }
            UtilityFunction::TypeOf => "Return the type name of a value",
            UtilityFunction::Version => "Return the GraphDB version string",
            UtilityFunction::CurrentUser => "Return the current user name",
            UtilityFunction::CurrentDatabase => "Return the current database name",
            UtilityFunction::Corr => "Return the correlation coefficient of two lists",
            UtilityFunction::CovarPop => "Return the population covariance of two lists",
            UtilityFunction::CovarSamp => "Return the sample covariance of two lists",
            UtilityFunction::OctetLength => "Return the number of bytes in a string or blob",
            UtilityFunction::Encode => "Encode a string to blob using encoding (UTF8, HEX, BASE64)",
            UtilityFunction::Decode => "Decode a blob to string using encoding (UTF8, HEX, BASE64)",
            UtilityFunction::UnionValue => "Create a union value with a tag and a value",
            UtilityFunction::UnionTag => "Get the tag of a union value",
            UtilityFunction::UnionExtract => "Extract the value from a union value",
        }
    }

    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if !self.is_variadic() && args.len() != self.arity() {
            return Err(ExpressionError::invalid_arity(
                self.name(),
                self.arity(),
                args.len(),
            ));
        }
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
            UtilityFunction::IfNull => execute_ifnull(args),
            UtilityFunction::TypeOf => execute_typeof(args),
            UtilityFunction::Version => execute_version(args),
            UtilityFunction::CurrentUser => execute_current_user(args),
            UtilityFunction::CurrentDatabase => execute_current_database(args),
            UtilityFunction::Corr => execute_corr(args),
            UtilityFunction::CovarPop => execute_covar_pop(args),
            UtilityFunction::CovarSamp => execute_covar_samp(args),
            UtilityFunction::OctetLength => execute_octet_length(args),
            UtilityFunction::Encode => execute_encode(args),
            UtilityFunction::Decode => execute_decode(args),
            UtilityFunction::UnionValue => execute_union_value(args),
            UtilityFunction::UnionTag => execute_union_tag(args),
            UtilityFunction::UnionExtract => execute_union_extract(args),
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
    if args[0].is_null() || args[1].is_null() {
        return Ok(Value::Null(NullType::Null));
    }
    let json_value = arg_to_json_value(&args[0])?;
    let path = match &args[1] {
        Value::String(s) => s.as_str(),
        _ => {
            return Err(ExpressionError::type_error(
                "json_extract path must be a string",
            ))
        }
    };
    let result = extract_json_value(&json_value, path);
    Ok(json_to_value(result))
}

fn execute_json_build_object(args: &[Value]) -> Result<Value, ExpressionError> {
    use serde_json::Map;

    if !args.len().is_multiple_of(2) {
        return Err(ExpressionError::type_error(
            "json_build_object requires an even number of arguments (key-value pairs)",
        ));
    }

    let mut map = Map::new();
    for chunk in args.chunks(2) {
        let key = match &chunk[0] {
            Value::String(s) => s.to_string(),
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

    Ok(Value::string(
        serde_json::to_string(&JsonValue::Object(map))
            .map_err(|e| ExpressionError::type_error(format!("JSON serialization error: {}", e)))?,
    ))
}

fn execute_json_build_array(args: &[Value]) -> Result<Value, ExpressionError> {
    let arr: Vec<JsonValue> = args.iter().map(value_to_json_value).collect();
    Ok(Value::string(
        serde_json::to_string(&JsonValue::Array(arr))
            .map_err(|e| ExpressionError::type_error(format!("JSON serialization error: {}", e)))?,
    ))
}

fn execute_json_object_keys(args: &[Value]) -> Result<Value, ExpressionError> {
    use graphdb_core::value::list::List;

    if args[0].is_null() {
        return Ok(Value::Null(NullType::Null));
    }
    let json_value = arg_to_json_value(&args[0])?;

    match &json_value {
        JsonValue::Object(map) => {
            let keys: Vec<Value> = map.keys().map(|k| Value::string(k.clone())).collect();
            Ok(Value::list(List { values: keys }))
        }
        _ => Err(ExpressionError::type_error(
            "json_object_keys requires a JSON object",
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
        Value::String(s) => JsonValue::String(s.to_string()),
        Value::List(list) => {
            JsonValue::Array(list.values.iter().map(value_to_json_value).collect())
        }
        Value::Map(map) => {
            let obj: serde_json::Map<String, JsonValue> = map
                .iter()
                .map(|(k, v)| {
                    // JSON object keys must be strings; non-string keys use
                    // their readable serialization.
                    let key = match k {
                        Value::String(s) => s.to_string(),
                        other => format!("{}", other),
                    };
                    (key, value_to_json_value(v))
                })
                .collect();
            JsonValue::Object(obj)
        }
        Value::Json(j) => j.to_value().unwrap_or(JsonValue::Null),
        Value::JsonB(j) => j.as_value().clone(),
        _ => JsonValue::Null,
    }
}

/// Extract a serde_json::Value from any JSON-compatible value type.
fn arg_to_json_value(arg: &Value) -> Result<JsonValue, ExpressionError> {
    match arg {
        Value::Null(_) => Err(ExpressionError::type_error("Null JSON value")),
        Value::String(s) => {
            serde_json::from_str(s).map_err(|_| ExpressionError::type_error("Invalid JSON string"))
        }
        Value::Json(j) => j
            .to_value()
            .map_err(|e| ExpressionError::type_error(format!("Invalid JSON: {}", e))),
        Value::JsonB(j) => Ok(j.as_value().clone()),
        _ => Err(ExpressionError::type_error(
            "Expected a JSON-compatible value (string, json, or jsonb)",
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
        JsonValue::String(s) => Value::string(s.clone()),
        JsonValue::Array(arr) => {
            let values: Vec<Value> = arr.iter().map(json_to_value).collect();
            Value::list(List { values })
        }
        JsonValue::Object(obj) => {
            let map: std::collections::HashMap<Value, Value> = obj
                .iter()
                .map(|(k, v)| (Value::string(k.clone()), json_to_value(v)))
                .collect();
            Value::map(map)
        }
    }
}

fn execute_nullif(args: &[Value]) -> Result<Value, ExpressionError> {
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
    use graphdb_core::value::uuid::UuidValue;
    Ok(Value::Uuid(UuidValue::new_v4()))
}

fn execute_json_each(args: &[Value]) -> Result<Value, ExpressionError> {
    if args[0].is_null() {
        return Ok(Value::Null(NullType::Null));
    }
    let json_value = arg_to_json_value(&args[0])?;

    match json_value {
        JsonValue::Object(map) => {
            let entries: Vec<Value> = map
                .into_iter()
                .map(|(k, v)| {
                    Value::list(List {
                        values: vec![Value::string(k), json_to_value(&v)],
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

fn execute_json_typeof(args: &[Value]) -> Result<Value, ExpressionError> {
    if args[0].is_null() {
        return Ok(Value::Null(NullType::Null));
    }
    let json_value = arg_to_json_value(&args[0])?;

    let type_str = match &json_value {
        JsonValue::Null => "null",
        JsonValue::Bool(_) => "boolean",
        JsonValue::Number(_) => "number",
        JsonValue::String(_) => "string",
        JsonValue::Array(_) => "array",
        JsonValue::Object(_) => "object",
    };
    Ok(Value::string(type_str))
}

fn execute_json_strip_nulls(args: &[Value]) -> Result<Value, ExpressionError> {
    if args[0].is_null() {
        return Ok(Value::Null(NullType::Null));
    }
    let json_value = arg_to_json_value(&args[0])?;
    let stripped = strip_nulls_from_json(json_value);
    let output = serde_json::to_string(&stripped)
        .map_err(|e| ExpressionError::type_error(format!("JSON serialization error: {}", e)))?;
    Ok(Value::string(output))
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

fn execute_ifnull(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Null(_) => Ok(args[1].clone()),
        other => Ok(other.clone()),
    }
}

fn execute_typeof(args: &[Value]) -> Result<Value, ExpressionError> {
    let type_name = match &args[0] {
        Value::Null(_) => "null",
        Value::Bool(_) => "bool",
        Value::SmallInt(_) => "smallint",
        Value::Int(_) => "int",
        Value::BigInt(_) => "bigint",
        Value::Float(_) => "float",
        Value::Double(_) => "double",
        Value::String(_) => "string",
        Value::Date(_) => "date",
        Value::Time(_) => "time",
        Value::DateTime(_) => "datetime",
        Value::List(_) => "list",
        Value::Map(_) => "map",
        Value::Set(_) => "set",
        Value::Vertex(_) => "vertex",
        Value::Edge(_) => "edge",
        Value::Path(_) => "path",
        Value::Uuid(_) => "uuid",
        Value::Vector(_) => "vector",
        Value::Geography(_) => "geography",
        _ => "unknown",
    };
    Ok(Value::string(type_name))
}

fn execute_version(_args: &[Value]) -> Result<Value, ExpressionError> {
    Ok(Value::string("GraphDB 0.1.0"))
}

fn execute_current_user(_args: &[Value]) -> Result<Value, ExpressionError> {
    Ok(Value::string("root"))
}

fn execute_current_database(_args: &[Value]) -> Result<Value, ExpressionError> {
    Ok(Value::string("default"))
}

fn extract_numeric_pairs(args: &[Value]) -> Result<(Vec<f64>, Vec<f64>), ExpressionError> {
    let xs = match &args[0] {
        Value::List(list) => list
            .values
            .iter()
            .map(|v| match v {
                Value::SmallInt(i) => Ok(*i as f64),
                Value::Int(i) => Ok(*i as f64),
                Value::BigInt(i) => Ok(*i as f64),
                Value::Float(f) => Ok(*f as f64),
                Value::Double(f) => Ok(*f),
                Value::Null(_) => Ok(f64::NAN),
                _ => Err(ExpressionError::type_error("Non-numeric value in list")),
            })
            .collect::<Result<Vec<f64>, _>>(),
        Value::Null(_) => return Ok((vec![], vec![])),
        _ => return Err(ExpressionError::type_error("First argument must be a list")),
    }?;
    let ys = match &args[1] {
        Value::List(list) => list
            .values
            .iter()
            .map(|v| match v {
                Value::SmallInt(i) => Ok(*i as f64),
                Value::Int(i) => Ok(*i as f64),
                Value::BigInt(i) => Ok(*i as f64),
                Value::Float(f) => Ok(*f as f64),
                Value::Double(f) => Ok(*f),
                Value::Null(_) => Ok(f64::NAN),
                _ => Err(ExpressionError::type_error("Non-numeric value in list")),
            })
            .collect::<Result<Vec<f64>, _>>(),
        Value::Null(_) => return Ok((vec![], vec![])),
        _ => {
            return Err(ExpressionError::type_error(
                "Second argument must be a list",
            ))
        }
    }?;
    if xs.len() != ys.len() {
        return Err(ExpressionError::type_error(
            "Lists must have the same length",
        ));
    }
    Ok((xs, ys))
}

fn execute_corr(args: &[Value]) -> Result<Value, ExpressionError> {
    let (xs, ys) = extract_numeric_pairs(args)?;
    if xs.is_empty() {
        return Ok(Value::Null(NullType::Null));
    }
    let n = xs.len() as f64;
    let sum_x: f64 = xs.iter().filter(|v| !v.is_nan()).sum();
    let sum_y: f64 = ys.iter().filter(|v| !v.is_nan()).sum();
    let sum_xy: f64 = xs
        .iter()
        .zip(ys.iter())
        .filter(|(x, y)| !x.is_nan() && !y.is_nan())
        .map(|(x, y)| x * y)
        .sum();
    let sum_x2: f64 = xs.iter().filter(|v| !v.is_nan()).map(|x| x * x).sum();
    let sum_y2: f64 = ys.iter().filter(|v| !v.is_nan()).map(|y| y * y).sum();
    let numerator = n * sum_xy - sum_x * sum_y;
    let denom = (n * sum_x2 - sum_x * sum_x).sqrt() * (n * sum_y2 - sum_y * sum_y).sqrt();
    if denom == 0.0 {
        Ok(Value::Null(NullType::Null))
    } else {
        Ok(Value::Double(numerator / denom))
    }
}

fn execute_covar_pop(args: &[Value]) -> Result<Value, ExpressionError> {
    let (xs, ys) = extract_numeric_pairs(args)?;
    if xs.is_empty() {
        return Ok(Value::Null(NullType::Null));
    }
    let n = xs.len() as f64;
    let mean_x: f64 = xs.iter().sum::<f64>() / n;
    let mean_y: f64 = ys.iter().sum::<f64>() / n;
    let covar: f64 = xs
        .iter()
        .zip(ys.iter())
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum::<f64>()
        / n;
    Ok(Value::Double(covar))
}

fn execute_covar_samp(args: &[Value]) -> Result<Value, ExpressionError> {
    let (xs, ys) = extract_numeric_pairs(args)?;
    if xs.len() < 2 {
        return Ok(Value::Null(NullType::Null));
    }
    let n = xs.len() as f64;
    let mean_x: f64 = xs.iter().sum::<f64>() / n;
    let mean_y: f64 = ys.iter().sum::<f64>() / n;
    let covar: f64 = xs
        .iter()
        .zip(ys.iter())
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum::<f64>()
        / (n - 1.0);
    Ok(Value::Double(covar))
}

fn execute_octet_length(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(s) => Ok(Value::BigInt(s.len() as i64)),
        Value::Blob(b) => Ok(Value::BigInt(b.len() as i64)),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "octet_length requires a string or blob type",
        )),
    }
}

fn execute_encode(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(s) => Ok(Value::Blob(s.as_bytes().to_vec())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("encode requires a string type")),
    }
}

fn execute_decode(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Blob(b) => {
            let s = String::from_utf8(b.to_vec()).map_err(|_| {
                ExpressionError::type_error("decode failed: invalid UTF-8 sequence")
            })?;
            Ok(Value::string(s))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error("decode requires a blob type")),
    }
}

fn execute_union_value(args: &[Value]) -> Result<Value, ExpressionError> {
    let tag = match &args[0] {
        Value::Int(i) => *i,
        Value::SmallInt(i) => *i as i32,
        Value::BigInt(i) => *i as i32,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "union_value first argument must be an integer tag",
            ))
        }
    };
    if tag < 0 {
        return Err(ExpressionError::type_error(
            "union tag must be non-negative",
        ));
    }
    let value = args[1].clone();
    let mut map = HashMap::new();
    map.insert(Value::string("__tag"), Value::Int(tag));
    map.insert(Value::string("__value"), value);
    Ok(Value::map(map))
}

fn execute_union_tag(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Map(m) => {
            let tag_key = Value::string("__tag");
            match m.get(&tag_key) {
                Some(Value::Int(tag)) => Ok(Value::Int(*tag)),
                Some(Value::SmallInt(tag)) => Ok(Value::Int(*tag as i32)),
                Some(Value::BigInt(tag)) => Ok(Value::Int(*tag as i32)),
                Some(_) => Err(ExpressionError::type_error("union tag must be an integer")),
                None => Err(ExpressionError::type_error(
                    "not a union value: missing __tag",
                )),
            }
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "union_tag requires a union value (map)",
        )),
    }
}

fn execute_union_extract(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Map(m) => {
            let value_key = Value::string("__value");
            match m.get(&value_key) {
                Some(v) => Ok(v.clone()),
                None => Err(ExpressionError::type_error(
                    "not a union value: missing __value",
                )),
            }
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "union_extract requires a union value (map)",
        )),
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
            .execute(&[Value::string("test")])
            .expect("Execution should succeed");
        assert!(matches!(result, Value::BigInt(_)));
    }

    #[test]
    fn test_ifnull() {
        assert_eq!(
            UtilityFunction::IfNull
                .execute(&[Value::Null(NullType::Null), Value::Int(42)])
                .unwrap(),
            Value::Int(42)
        );
        assert_eq!(
            UtilityFunction::IfNull
                .execute(&[Value::Int(10), Value::Int(42)])
                .unwrap(),
            Value::Int(10)
        );
    }

    #[test]
    fn test_typeof() {
        assert_eq!(
            UtilityFunction::TypeOf.execute(&[Value::Int(1)]).unwrap(),
            Value::string("int")
        );
        assert_eq!(
            UtilityFunction::TypeOf
                .execute(&[Value::string("hello")])
                .unwrap(),
            Value::string("string")
        );
        assert_eq!(
            UtilityFunction::TypeOf
                .execute(&[Value::Bool(true)])
                .unwrap(),
            Value::string("bool")
        );
        assert_eq!(
            UtilityFunction::TypeOf
                .execute(&[Value::Null(NullType::Null)])
                .unwrap(),
            Value::string("null")
        );
    }

    #[test]
    fn test_version() {
        let result = UtilityFunction::Version.execute(&[]).unwrap();
        assert!(matches!(result, Value::String(_)));
    }

    #[test]
    fn test_current_user() {
        let result = UtilityFunction::CurrentUser.execute(&[]).unwrap();
        assert!(matches!(result, Value::String(_)));
    }

    #[test]
    fn test_current_database() {
        let result = UtilityFunction::CurrentDatabase.execute(&[]).unwrap();
        assert!(matches!(result, Value::String(_)));
    }

    #[test]
    fn test_corr() {
        let xs = Value::list(List {
            values: vec![
                Value::Double(1.0),
                Value::Double(2.0),
                Value::Double(3.0),
                Value::Double(4.0),
                Value::Double(5.0),
            ],
        });
        let ys = Value::list(List {
            values: vec![
                Value::Double(2.0),
                Value::Double(4.0),
                Value::Double(6.0),
                Value::Double(8.0),
                Value::Double(10.0),
            ],
        });
        let result = UtilityFunction::Corr
            .execute(&[xs, ys])
            .expect("corr should succeed");
        assert!(matches!(result, Value::Double(v) if (v - 1.0).abs() < 1e-10));
    }

    #[test]
    fn test_covar_pop() {
        let xs = Value::list(List {
            values: vec![Value::Double(1.0), Value::Double(2.0)],
        });
        let ys = Value::list(List {
            values: vec![Value::Double(3.0), Value::Double(4.0)],
        });
        let result = UtilityFunction::CovarPop
            .execute(&[xs, ys])
            .expect("covar_pop should succeed");
        assert!(matches!(result, Value::Double(_)));
    }

    #[test]
    fn test_octet_length() {
        let result = UtilityFunction::OctetLength
            .execute(&[Value::string("hello")])
            .expect("octet_length should succeed");
        assert_eq!(result, Value::Int(5));
    }

    #[test]
    fn test_encode() {
        let result = UtilityFunction::Encode
            .execute(&[Value::string("hello")])
            .expect("encode should succeed");
        assert!(matches!(result, Value::Blob(_)));
    }

    #[test]
    fn test_decode() {
        let blob = Value::Blob(b"hello".to_vec());
        let result = UtilityFunction::Decode
            .execute(&[blob])
            .expect("decode should succeed");
        assert_eq!(result, Value::string("hello"));
    }

    #[test]
    fn test_union_value() {
        let result = UtilityFunction::UnionValue
            .execute(&[Value::Int(0), Value::Int(42)])
            .expect("union_value should succeed");
        match &result {
            Value::Map(m) => {
                let tag = m.get(&Value::string("__tag")).expect("missing __tag");
                assert_eq!(tag, &Value::Int(0));
                let val = m.get(&Value::string("__value")).expect("missing __value");
                assert_eq!(val, &Value::Int(42));
            }
            _ => panic!("expected map"),
        }
    }

    #[test]
    fn test_union_value_null_propagation() {
        let result = UtilityFunction::UnionValue
            .execute(&[Value::Null(NullType::Null), Value::Int(42)])
            .expect("union_value should succeed");
        assert_eq!(result, Value::Null(NullType::Null));
    }

    #[test]
    fn test_union_value_negative_tag() {
        let result = UtilityFunction::UnionValue.execute(&[Value::Int(-1), Value::Int(42)]);
        assert!(result.is_err());
    }

    #[test]
    fn test_union_tag() {
        let mut map = HashMap::new();
        map.insert(Value::string("__tag"), Value::Int(2));
        map.insert(Value::string("__value"), Value::string("hello"));
        let union_val = Value::map(map);
        let result = UtilityFunction::UnionTag
            .execute(&[union_val])
            .expect("union_tag should succeed");
        assert_eq!(result, Value::Int(2));
    }

    #[test]
    fn test_union_tag_missing_key() {
        let map = HashMap::new();
        let union_val = Value::map(map);
        let result = UtilityFunction::UnionTag.execute(&[union_val]);
        assert!(result.is_err());
    }

    #[test]
    fn test_union_tag_null() {
        let result = UtilityFunction::UnionTag
            .execute(&[Value::Null(NullType::Null)])
            .expect("union_tag should succeed");
        assert_eq!(result, Value::Null(NullType::Null));
    }

    #[test]
    fn test_union_extract() {
        let mut map = HashMap::new();
        map.insert(Value::string("__tag"), Value::Int(0));
        map.insert(Value::string("__value"), Value::string("hello"));
        let union_val = Value::map(map);
        let result = UtilityFunction::UnionExtract
            .execute(&[union_val])
            .expect("union_extract should succeed");
        assert_eq!(result, Value::string("hello"));
    }

    #[test]
    fn test_union_extract_missing_key() {
        let map = HashMap::new();
        let union_val = Value::map(map);
        let result = UtilityFunction::UnionExtract.execute(&[union_val]);
        assert!(result.is_err());
    }

    #[test]
    fn test_union_extract_null() {
        let result = UtilityFunction::UnionExtract
            .execute(&[Value::Null(NullType::Null)])
            .expect("union_extract should succeed");
        assert_eq!(result, Value::Null(NullType::Null));
    }

    #[test]
    fn test_union_roundtrip() {
        let u = UtilityFunction::UnionValue
            .execute(&[Value::Int(1), Value::Double(3.14)])
            .expect("create union");
        let tag = UtilityFunction::UnionTag
            .execute(&[u.clone()])
            .expect("get tag");
        assert_eq!(tag, Value::Int(1));
        let val = UtilityFunction::UnionExtract
            .execute(&[u])
            .expect("extract value");
        assert_eq!(val, Value::Double(3.14));
    }

    #[test]
    fn test_octet_length_counts_bytes() {
        // Two Chinese characters are 6 bytes in UTF-8.
        let result = UtilityFunction::OctetLength
            .execute(&[Value::string("你好")])
            .expect("octet_length should succeed");
        assert_eq!(result, Value::BigInt(6));
        let result = UtilityFunction::OctetLength
            .execute(&[Value::string("hello")])
            .expect("octet_length should succeed");
        assert_eq!(result, Value::BigInt(5));
    }

    #[test]
    fn test_length_size_octet_length_contrast() {
        use crate::executor::expression::functions::{ContainerFunction, PathFunction};
        // 5 ASCII chars + 2 Chinese chars: 7 chars, 5 + 6 = 11 bytes.
        let s = Value::string("hello你好");
        let length = PathFunction::PathLength
            .execute(std::slice::from_ref(&s))
            .expect("length should succeed");
        let size = ContainerFunction::Size
            .execute(std::slice::from_ref(&s))
            .expect("size should succeed");
        let octets = UtilityFunction::OctetLength
            .execute(std::slice::from_ref(&s))
            .expect("octet_length should succeed");
        assert_eq!(length, Value::BigInt(7));
        assert_eq!(size, Value::BigInt(7));
        assert_eq!(octets, Value::BigInt(11));
    }

    #[test]
    fn test_utility_arity() {
        use crate::executor::expression::ExpressionErrorType;
        let err = UtilityFunction::OctetLength.execute(&[]).unwrap_err();
        assert_eq!(err.error_type, ExpressionErrorType::InvalidArgumentCount);
        assert!(err.message.contains("octet_length"));
    }
}
