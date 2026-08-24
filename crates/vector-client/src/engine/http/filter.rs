use serde_json::{json, Value};

use crate::error::Result;
use crate::types::*;

use super::super::common::filter::{
    classify_match_any, classify_match_value, process_filter, ClassifiedMatch, ClassifiedMatchAny,
    ConditionHandler,
};

pub fn convert_filter(filter: &VectorFilter) -> Result<Option<Value>> {
    let handler = JsonConditionHandler;
    process_filter(filter, &handler)
}

struct JsonConditionHandler;

impl ConditionHandler for JsonConditionHandler {
    type Condition = Value;
    type Filter = Value;

    fn handle_match(&self, field: &str, value: &str) -> Value {
        // Typed matching mirrors the gRPC translation so both transports
        // evaluate identically against real Qdrant payloads.
        let match_body = match classify_match_value(value) {
            ClassifiedMatch::Integer(i) => json!({ "value": i }),
            ClassifiedMatch::Boolean(b) => json!({ "value": b }),
            ClassifiedMatch::Keyword(k) => json!({ "value": k }),
        };
        json!({
            "key": field,
            "match": match_body
        })
    }

    fn handle_match_any(&self, field: &str, values: &[serde_json::Value]) -> Value {
        // Element types are preserved exactly like the gRPC translation.
        match classify_match_any(values) {
            ClassifiedMatchAny::Integers(integers) => json!({
                "key": field,
                "match": { "any": integers }
            }),
            ClassifiedMatchAny::Strings(strings) => json!({
                "key": field,
                "match": { "any": strings }
            }),
            ClassifiedMatchAny::Booleans(booleans) => {
                // Mirror the gRPC translation: upstream Match has no
                // repeated-boolean variant, so the list becomes an OR over
                // singular boolean matches inside a bare filter condition.
                let should: Vec<Value> = booleans
                    .into_iter()
                    .map(|b| json!({ "key": field, "match": { "value": b } }))
                    .collect();
                json!({ "filter": { "should": should } })
            }
        }
    }

    fn handle_range(&self, field: &str, range: &RangeCondition) -> Value {
        let mut range_obj = serde_json::Map::new();
        if let Some(v) = range.gt {
            range_obj.insert("gt".to_string(), json!(v));
        }
        if let Some(v) = range.gte {
            range_obj.insert("gte".to_string(), json!(v));
        }
        if let Some(v) = range.lt {
            range_obj.insert("lt".to_string(), json!(v));
        }
        if let Some(v) = range.lte {
            range_obj.insert("lte".to_string(), json!(v));
        }
        json!({
            "key": field,
            "range": range_obj
        })
    }

    fn handle_is_empty(&self, field: &str) -> Value {
        json!({ "is_empty": { "key": field } })
    }

    fn handle_is_null(&self, field: &str) -> Value {
        json!({ "is_null": { "key": field } })
    }

    fn handle_has_id(&self, ids: &[String]) -> Value {
        let point_ids: Vec<Value> = ids
            .iter()
            .map(|id| crate::engine::common::utils::point_id_to_json(id))
            .collect();
        json!({ "has_id": point_ids })
    }

    fn handle_geo_radius(&self, field: &str, radius: &GeoRadius) -> Value {
        json!({
            "key": field,
            "geo_radius": {
                "center": { "lat": radius.center.lat, "lon": radius.center.lon },
                "radius": radius.radius as f32
            }
        })
    }

    fn handle_geo_bounding_box(&self, field: &str, bbox: &GeoBoundingBox) -> Value {
        json!({
            "key": field,
            "geo_bounding_box": {
                "top_left": { "lat": bbox.top_left.lat, "lon": bbox.top_left.lon },
                "bottom_right": { "lat": bbox.bottom_right.lat, "lon": bbox.bottom_right.lon }
            }
        })
    }

    fn handle_values_count(&self, field: &str, count: &ValuesCountCondition) -> Value {
        let mut count_obj = serde_json::Map::new();
        if let Some(v) = count.gt {
            count_obj.insert("gt".to_string(), json!(v));
        }
        if let Some(v) = count.gte {
            count_obj.insert("gte".to_string(), json!(v));
        }
        if let Some(v) = count.lt {
            count_obj.insert("lt".to_string(), json!(v));
        }
        if let Some(v) = count.lte {
            count_obj.insert("lte".to_string(), json!(v));
        }
        json!({
            "key": field,
            "values_count": count_obj
        })
    }

    fn handle_contains(&self, field: &str, value: &str) -> Value {
        json!({
            "key": field,
            "match": { "text": value }
        })
    }

    fn handle_nested(&self, field: &str, filter: Value) -> Value {
        json!({
            "nested": {
                "key": field,
                "filter": filter
            }
        })
    }

    fn build_filter(
        &self,
        must: Vec<Value>,
        must_not: Vec<Value>,
        should: Vec<Value>,
        min_should: Option<(Vec<Value>, usize)>,
    ) -> Option<Value> {
        let mut filter_obj = serde_json::Map::new();

        if !must.is_empty() {
            filter_obj.insert("must".to_string(), Value::Array(must));
        }
        if !must_not.is_empty() {
            filter_obj.insert("must_not".to_string(), Value::Array(must_not));
        }

        if !should.is_empty() {
            filter_obj.insert("should".to_string(), Value::Array(should));
        }

        if let Some((conditions, min_count)) = min_should {
            // min_should carries its own condition list; serializing the
            // count in its place breaks semantics. `should` and `min_should`
            // are independent clauses and must both survive translation
            // (the gRPC translator keeps both as well).
            filter_obj.insert(
                "min_should".to_string(),
                json!({ "conditions": conditions, "min_count": min_count }),
            );
        }

        if filter_obj.is_empty() {
            None
        } else {
            Some(Value::Object(filter_obj))
        }
    }
}
