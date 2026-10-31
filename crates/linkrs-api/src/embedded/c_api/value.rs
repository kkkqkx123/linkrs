use crate::embedded::c_api::types::linkrs_value_t;
use linkrs_core::{List, Value};

/// Convert a C value to a core Value.
///
/// # Safety
/// - `value` must be a valid pointer to a linkrs_value_t
pub unsafe fn linkrs_value_to_core(value: *const linkrs_value_t) -> Value {
    super::query::convert_c_value_to_rust(&*value)
}

/// Encode a core [`Value`] into the JSON shape used by the wire protocols.
///
/// Scalar, list, set, and map variants map to their natural JSON form;
/// everything else falls back to the derived serde representation so no
/// variant is silently dropped.
pub(crate) fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Empty | Value::Null(_) => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::SmallInt(i) => serde_json::Value::from(*i),
        Value::Int(i) => serde_json::Value::from(*i),
        Value::BigInt(i) => serde_json::Value::from(*i),
        Value::Float(f) => serde_json::Number::from_f64(*f as f64)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Double(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::String(s) => serde_json::Value::String(s.to_string()),
        Value::FixedString(s) => serde_json::Value::String(s.clone()),
        Value::List(list) => {
            serde_json::Value::Array(list.iter().map(value_to_json).collect::<Vec<_>>())
        }
        Value::Set(set) => {
            serde_json::Value::Array(set.iter().map(value_to_json).collect::<Vec<_>>())
        }
        Value::Map(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (format!("{k}"), value_to_json(v)))
                .collect(),
        ),
        Value::Json(j) => serde_json::from_str(j.as_str()).unwrap_or(serde_json::Value::Null),
        Value::JsonB(j) => j.as_value().clone(),
        other => serde_json::to_value(other).unwrap_or(serde_json::Value::Null),
    }
}

/// Decode a JSON value into a core [`Value`], mirroring the query parameter
/// binding used by the HTTP and gRPC transports.
pub(crate) fn json_to_value(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Null(Default::default()),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::BigInt(i)
            } else if let Some(u) = n.as_u64() {
                Value::BigInt(u as i64)
            } else {
                Value::Double(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => Value::string(s.as_str()),
        serde_json::Value::Array(items) => {
            Value::list(List::from_vec(items.iter().map(json_to_value).collect()))
        }
        serde_json::Value::Object(object) => {
            Value::string_map(object.iter().map(|(k, v)| (k.as_str(), json_to_value(v))))
        }
    }
}
