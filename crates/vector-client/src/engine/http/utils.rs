use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
pub struct QdrantSearchResult {
    pub id: crate::types::PointId,
    pub score: f32,
    #[serde(default)]
    pub payload: Option<Value>,
    #[serde(default)]
    pub vector: Option<VectorValue>,
}

#[derive(Deserialize)]
#[serde(untagged)]
pub enum VectorValue {
    Single(Vec<f32>),
    Multi { data: Vec<f32> },
    Named(HashMap<String, Value>),
}

impl VectorValue {
    pub fn into_vec(self) -> Option<Vec<f32>> {
        match self {
            VectorValue::Single(v) => Some(v),
            VectorValue::Multi { data } => Some(data),
            VectorValue::Named(named) => {
                if named.len() == 1 {
                    named.into_values().next().and_then(value_to_vec)
                } else {
                    None
                }
            }
        }
    }
}

#[derive(Deserialize)]
pub struct QdrantUpsertResult {
    pub operation_id: Option<u64>,
    pub status: Option<String>,
}

pub fn parse_payload(payload: Option<Value>) -> Option<HashMap<String, Value>> {
    payload.and_then(|v| match v {
        Value::Object(map) => {
            let result: HashMap<String, Value> = map.into_iter().collect();
            Some(result)
        }
        _ => None,
    })
}

fn value_to_vec(value: Value) -> Option<Vec<f32>> {
    match value {
        Value::Array(values) => values
            .into_iter()
            .map(|entry| match entry {
                Value::Number(number) => number.as_f64().map(|v| v as f32),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_value_into_vec_single() {
        let vv = VectorValue::Single(vec![1.0, 2.0, 3.0]);
        assert_eq!(vv.into_vec(), Some(vec![1.0, 2.0, 3.0]));
    }

    #[test]
    fn test_vector_value_into_vec_multi() {
        let vv = VectorValue::Multi {
            data: vec![4.0, 5.0],
        };
        assert_eq!(vv.into_vec(), Some(vec![4.0, 5.0]));
    }

    #[test]
    fn test_vector_value_into_vec_named_single_entry() {
        let mut map = HashMap::new();
        map.insert("default".into(), serde_json::json!([1.0, 2.0]));
        let vv = VectorValue::Named(map);
        assert_eq!(vv.into_vec(), Some(vec![1.0, 2.0]));
    }

    #[test]
    fn test_vector_value_into_vec_named_multi_entry() {
        let mut map = HashMap::new();
        map.insert("a".into(), serde_json::json!([1.0]));
        map.insert("b".into(), serde_json::json!([2.0]));
        let vv = VectorValue::Named(map);
        // Named with >1 entries returns None
        assert!(vv.into_vec().is_none());
    }

    #[test]
    fn test_parse_payload_object() {
        let val = serde_json::json!({"key": "value", "num": 42});
        let parsed = parse_payload(Some(val));
        assert!(parsed.is_some());
        let map = parsed.unwrap();
        assert_eq!(map.get("key").and_then(|v| v.as_str()), Some("value"));
        assert_eq!(map.get("num").and_then(|v| v.as_i64()), Some(42));
    }

    #[test]
    fn test_parse_payload_non_object() {
        let val = serde_json::json!([1, 2, 3]);
        let parsed = parse_payload(Some(val));
        assert!(parsed.is_none());
    }

    #[test]
    fn test_parse_payload_none() {
        let parsed = parse_payload(None);
        assert!(parsed.is_none());
    }

    #[test]
    fn test_qdrant_search_result_single_vector() {
        let json = serde_json::json!({
            "id": 1,
            "score": 0.95,
            "payload": {"color": "red"},
            "vector": [0.1, 0.2]
        });
        let result: QdrantSearchResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.id, crate::types::PointId::Num(1));
        assert!((result.score - 0.95).abs() < f32::EPSILON);
        assert!(result.payload.is_some());
        assert!(result.vector.is_some());
    }

    #[test]
    fn test_qdrant_upsert_result_deserialize() {
        let json = serde_json::json!({
            "operation_id": 12345,
            "status": "completed"
        });
        let result: QdrantUpsertResult = serde_json::from_value(json).unwrap();
        assert_eq!(result.operation_id, Some(12345));
        assert_eq!(result.status.as_deref(), Some("completed"));
    }

    #[test]
    fn test_qdrant_upsert_result_minimal() {
        let json = serde_json::json!({});
        let result: QdrantUpsertResult = serde_json::from_value(json).unwrap();
        assert!(result.operation_id.is_none());
        assert!(result.status.is_none());
    }
}
