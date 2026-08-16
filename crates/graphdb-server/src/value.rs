//! `Value` ↔ `serde_json::Value` codec.
//!
//! Single canonical implementation of the wire encoding for graph
//! [`Value`]s, previously duplicated between the HTTP query handler and the
//! SSE stream handler. The JSON shape is the one the streaming handler
//! already emitted (complete variant coverage; `Vertex`/`Edge`/`Path` are
//! serialized through their own `Serialize` impls).

use graphdb_core::core::{List, Value};

/// Encode a core [`Value`] into its JSON wire representation.
pub fn to_json(value: Value) -> serde_json::Value {
    match value {
        Value::Empty => serde_json::Value::Null,
        Value::Null(_) => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(b),
        Value::SmallInt(i) => serde_json::Value::Number(i.into()),
        Value::Int(i) => serde_json::Value::Number(i.into()),
        Value::BigInt(i) => serde_json::Value::Number(i.into()),
        Value::Float(f) => serde_json::Value::Number(
            serde_json::Number::from_f64(f as f64).unwrap_or(serde_json::Number::from(0)),
        ),
        Value::Double(f) => serde_json::Value::Number(
            serde_json::Number::from_f64(f).unwrap_or(serde_json::Number::from(0)),
        ),
        Value::Decimal128(d) => serde_json::Value::String(d.to_string()),
        Value::String(s) => serde_json::Value::String(s.to_string()),
        Value::FixedString { data, .. } => serde_json::Value::String(data.to_string()),
        Value::Blob(blob) => serde_json::Value::String(format!("{:?}", blob)),
        Value::Date(d) => serde_json::Value::String(d.to_string()),
        Value::Time(t) => serde_json::Value::String(t.to_string()),
        Value::DateTime(dt) => serde_json::Value::String(dt.to_string()),
        Value::Vertex(v) => serde_json::json!(v),
        Value::Edge(e) => serde_json::json!(e),
        Value::Path(p) => serde_json::json!(p),
        Value::List(list) => serde_json::Value::Array(list.into_iter().map(to_json).collect()),
        Value::Map(map) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .into_iter()
                .map(|(k, v)| (format!("{}", k), to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        Value::Set(set) => serde_json::Value::Array(set.into_iter().map(to_json).collect()),
        Value::Geography(g) => serde_json::json!(g),
        Value::Vector(v) => {
            let arr = v
                .to_dense()
                .iter()
                .map(|&f| {
                    serde_json::Number::from_f64(f as f64).unwrap_or(serde_json::Number::from(0))
                })
                .collect::<Vec<_>>();
            serde_json::Value::Array(arr.into_iter().map(serde_json::Value::Number).collect())
        }
        Value::DataSet(ds) => serde_json::json!(ds),
        Value::Json(j) => serde_json::from_str(j.as_str()).unwrap_or(serde_json::Value::Null),
        Value::JsonB(j) => j.as_value().clone(),
        Value::Uuid(u) => serde_json::Value::String(u.to_hyphenated_string()),
        Value::Interval(i) => serde_json::Value::String(i.to_postgresql()),
        Value::VertexId(vid) => serde_json::Value::String(format!("{:?}", vid)),
        Value::EdgeId(eid) => serde_json::Value::String(format!("{:?}", eid)),
        Value::Struct(s) => {
            let obj: serde_json::Map<String, serde_json::Value> = s
                .fields
                .into_iter()
                .map(|(k, v)| (k, to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        Value::Array(a) => serde_json::Value::Array(a.values.into_iter().map(to_json).collect()),
    }
}

/// Decode a `serde_json::Value` (query parameter binding) into a core
/// [`Value`].
pub fn from_json(value: &serde_json::Value) -> Value {
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
            Value::list(List::from_vec(items.iter().map(from_json).collect()))
        }
        serde_json::Value::Object(map) => Value::Map(Box::new(
            map.iter()
                .map(|(k, v)| (Value::string(k.clone()), from_json(v)))
                .collect(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::core::types::DataType;

    #[test]
    fn scalars_encode() {
        assert_eq!(to_json(Value::Bool(true)), serde_json::json!(true));
        assert_eq!(to_json(Value::Int(7)), serde_json::json!(7));
        assert_eq!(to_json(Value::BigInt(-9)), serde_json::json!(-9));
        assert_eq!(to_json(Value::Double(1.5)), serde_json::json!(1.5));
        assert_eq!(to_json(Value::string("hi")), serde_json::json!("hi"));
        assert_eq!(to_json(Value::Null(Default::default())), serde_json::Value::Null);
        assert_eq!(to_json(Value::Empty), serde_json::Value::Null);
    }

    #[test]
    fn list_and_map_encode() {
        let list = Value::list(List::from_vec(vec![Value::Int(1), Value::Int(2)]));
        assert_eq!(to_json(list), serde_json::json!([1, 2]));

        let map: Value = Value::Map(Box::new(
            [(Value::string("a".to_string()), Value::BigInt(1))]
                .into_iter()
                .collect(),
        ));
        assert_eq!(to_json(map), serde_json::json!({"a": 1}));
    }

    #[test]
    fn scalar_roundtrip() {
        let cases = vec![
            serde_json::json!(null),
            serde_json::json!(true),
            serde_json::json!(42),
            serde_json::json!(-7),
            serde_json::json!(3.25),
            serde_json::json!("text"),
        ];
        for case in cases {
            let core = from_json(&case);
            let back = to_json(core);
            assert_eq!(back, case);
        }
    }

    #[test]
    fn list_roundtrip() {
        let case = serde_json::json!([1, "two", false, null]);
        let core = from_json(&case);
        assert!(matches!(core, Value::List(_)));
        assert_eq!(to_json(core), case);
    }

    #[test]
    fn map_roundtrip() {
        let case = serde_json::json!({"a": 1, "b": [true]});
        let core = from_json(&case);
        assert!(matches!(core, Value::Map(_)));
        assert_eq!(to_json(core), case);
    }

    #[test]
    fn data_type_display_is_wire_format() {
        assert_eq!(DataType::Bool.to_string(), "BOOL");
    }
}
