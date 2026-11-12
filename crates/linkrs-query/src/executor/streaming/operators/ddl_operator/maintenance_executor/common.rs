use crate::executor::streaming::chunk::ColumnInfo;
use linkrs_core::Value;

/// Column names for a header-less source: `col0..colN` matching the first
/// data row, so the schema never ends up narrower than the rows.
pub(crate) fn column_names_from_width(width: usize) -> Vec<String> {
    (0..width).map(|index| format!("col{index}")).collect()
}

pub(crate) fn columns_for(names: &[String]) -> Vec<ColumnInfo> {
    names
        .iter()
        .map(|name| ColumnInfo {
            name: name.clone(),
            data_type: "string".to_string(),
        })
        .collect()
}

pub(crate) fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null(linkrs_core::NullType::Null),
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::BigInt(i)
            } else if let Some(f) = n.as_f64() {
                Value::Double(f)
            } else {
                Value::string(n.to_string())
            }
        }
        // Strings stay strings: argument values are typed at plan time, so
        // re-parsing them here would turn `'123'` into 123.
        serde_json::Value::String(s) => Value::string(s.clone()),
        serde_json::Value::Array(arr) => {
            let values: Vec<Value> = arr.iter().map(json_to_value).collect();
            Value::list(linkrs_core::value::list::List::from(values))
        }
        serde_json::Value::Object(map) => {
            let entries: std::collections::HashMap<Value, Value> = map
                .iter()
                .map(|(k, v)| (Value::string(k.clone()), json_to_value(v)))
                .collect();
            Value::Map(Box::new(entries))
        }
    }
}
