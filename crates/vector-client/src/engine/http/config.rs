use serde_json::{json, Value};

use crate::types::{
    CompressionRatio, DistanceMetric, HnswConfig, IndexType, PayloadSchemaType, QuantizationConfig,
    QuantizationType,
};

pub fn distance_to_qdrant(distance: DistanceMetric) -> &'static str {
    match distance {
        DistanceMetric::Cosine => "Cosine",
        DistanceMetric::Euclid => "Euclid",
        DistanceMetric::Dot => "Dot",
        DistanceMetric::Manhattan => "Manhattan",
    }
}

pub fn field_type_to_qdrant(schema: PayloadSchemaType) -> &'static str {
    match schema {
        PayloadSchemaType::Keyword => "keyword",
        PayloadSchemaType::Integer => "integer",
        PayloadSchemaType::Float => "float",
        PayloadSchemaType::Text => "text",
        PayloadSchemaType::Bool => "bool",
        PayloadSchemaType::Geo => "geo",
        PayloadSchemaType::Datetime => "datetime",
    }
}

fn build_vectors_json(
    vector_size: usize,
    distance: DistanceMetric,
    index_type: Option<IndexType>,
    hnsw_config: &Option<HnswConfig>,
    quantization_config: &Option<QuantizationConfig>,
) -> Value {
    let distance_str = distance_to_qdrant(distance);
    let mut vectors = json!({
        "size": vector_size,
        "distance": distance_str
    });

    if let Some(ref hnsw) = hnsw_config {
        vectors["hnsw_config"] = build_hnsw_json(hnsw);
    }

    if let Some(s) = index_type {
        if s != IndexType::HNSW {
            tracing::warn!(
                "Index type {:?} not supported by Qdrant, using HNSW with config",
                s
            );
        }
    }

    if let Some(ref quant) = quantization_config {
        if quant.enabled {
            if let Some(ref qt) = quant.quant_type {
                vectors["quantization_config"] = build_quantization_json(qt);
            }
        }
    }

    vectors
}

fn build_hnsw_json(hnsw: &HnswConfig) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("m".to_string(), json!(hnsw.m));
    obj.insert("ef_construct".to_string(), json!(hnsw.ef_construct));
    if let Some(v) = hnsw.full_scan_threshold {
        obj.insert("full_scan_threshold".to_string(), json!(v));
    }
    if let Some(v) = hnsw.max_indexing_threads {
        obj.insert("max_indexing_threads".to_string(), json!(v));
    }
    if let Some(v) = hnsw.on_disk {
        obj.insert("on_disk".to_string(), json!(v));
    }
    if let Some(v) = hnsw.payload_m {
        obj.insert("payload_m".to_string(), json!(v));
    }
    Value::Object(obj)
}

fn build_quantization_json(qt: &QuantizationType) -> Value {
    let mut inner = serde_json::Map::new();
    let type_name = match qt {
        QuantizationType::Scalar { .. } => "scalar",
        QuantizationType::Product { .. } => "product",
        QuantizationType::Binary { .. } => "binary",
    };

    inner.insert("type".to_string(), json!(type_name));

    if let QuantizationType::Product { compression, .. } = qt {
        let ratio = match compression {
            CompressionRatio::X4 => 4,
            CompressionRatio::X8 => 8,
            CompressionRatio::X16 => 16,
            CompressionRatio::X32 => 32,
            CompressionRatio::X64 => 64,
        };
        inner.insert("compression".to_string(), json!(ratio));
    }

    let always_ram = match qt {
        QuantizationType::Scalar { always_ram, .. }
        | QuantizationType::Product { always_ram, .. }
        | QuantizationType::Binary { always_ram } => always_ram,
    };
    if let Some(v) = always_ram {
        inner.insert("always_ram".to_string(), json!(v));
    }

    if let QuantizationType::Scalar {
        quantile: Some(v), ..
    } = qt
    {
        inner.insert("quantile".to_string(), json!(v));
    }

    let mut outer = serde_json::Map::new();
    outer.insert(type_name.to_string(), Value::Object(inner));
    Value::Object(outer)
}

#[allow(clippy::too_many_arguments)]
pub fn build_create_collection_body(
    _name: &str,
    vector_size: usize,
    distance: DistanceMetric,
    index_type: Option<IndexType>,
    hnsw_config: &Option<HnswConfig>,
    quantization_config: &Option<QuantizationConfig>,
    on_disk_payload: Option<bool>,
    shard_number: Option<usize>,
    replication_factor: Option<usize>,
    write_consistency_factor: Option<usize>,
) -> Value {
    let vectors = build_vectors_json(
        vector_size,
        distance,
        index_type,
        hnsw_config,
        quantization_config,
    );

    let mut body = serde_json::Map::new();
    body.insert("vectors".to_string(), vectors);

    if let Some(on_disk) = on_disk_payload {
        body.insert("on_disk_payload".to_string(), json!(on_disk));
    }

    if let Some(shards) = shard_number {
        body.insert("shard_number".to_string(), json!(shards));
    }

    if let Some(rf) = replication_factor {
        body.insert("replication_factor".to_string(), json!(rf));
    }

    if let Some(wcf) = write_consistency_factor {
        body.insert("write_consistency_factor".to_string(), json!(wcf));
    }

    Value::Object(body)
}

pub fn build_upsert_body(points_json: Value) -> Value {
    json!({
        "points": points_json
    })
}

pub fn build_delete_by_ids_body(ids: Vec<Value>) -> Value {
    json!({
        "points": ids
    })
}

pub fn build_delete_by_filter_body(filter: Value) -> Value {
    json!({
        "filter": filter
    })
}

#[allow(clippy::too_many_arguments)]
pub fn build_search_body(
    vector: Vec<f32>,
    limit: usize,
    offset: Option<usize>,
    score_threshold: Option<f32>,
    filter_json: Option<Value>,
    with_payload: Option<bool>,
    with_vector: Option<bool>,
    nprobe: Option<usize>,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("vector".to_string(), json!(vector));
    body.insert("limit".to_string(), json!(limit));
    body.insert(
        "with_payload".to_string(),
        json!(with_payload.unwrap_or(true)),
    );
    body.insert(
        "with_vector".to_string(),
        json!(with_vector.unwrap_or(false)),
    );

    if let Some(off) = offset {
        body.insert("offset".to_string(), json!(off));
    }

    if let Some(threshold) = score_threshold {
        body.insert("score_threshold".to_string(), json!(threshold));
    }

    if let Some(ref filter) = filter_json {
        body.insert("filter".to_string(), filter.clone());
    }

    let mut params = serde_json::Map::new();
    if let Some(ef) = nprobe {
        params.insert("hnsw_ef".to_string(), json!(ef));
    }
    if !params.is_empty() {
        body.insert("params".to_string(), Value::Object(params));
    }

    Value::Object(body)
}

pub fn build_get_body(
    ids: Vec<Value>,
    with_payload: Option<bool>,
    with_vector: Option<bool>,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("ids".to_string(), json!(ids));
    body.insert(
        "with_payload".to_string(),
        json!(with_payload.unwrap_or(true)),
    );
    body.insert(
        "with_vector".to_string(),
        json!(with_vector.unwrap_or(false)),
    );
    Value::Object(body)
}

pub fn build_scroll_body(
    limit: usize,
    offset: Option<Value>,
    with_payload: Option<bool>,
    with_vector: Option<bool>,
) -> Value {
    let mut body = serde_json::Map::new();
    body.insert("limit".to_string(), json!(limit));
    body.insert(
        "with_payload".to_string(),
        json!(with_payload.unwrap_or(true)),
    );
    body.insert(
        "with_vector".to_string(),
        json!(with_vector.unwrap_or(false)),
    );

    if let Some(off) = offset {
        body.insert("offset".to_string(), off);
    }

    Value::Object(body)
}

pub fn build_set_payload_body(ids: Vec<Value>, payload: Value) -> Value {
    json!({
        "payload": payload,
        "points": ids
    })
}

pub fn build_delete_payload_body(ids: Vec<Value>, keys: Vec<String>) -> Value {
    json!({
        "keys": keys,
        "points": ids
    })
}

pub fn build_create_payload_index_body(field_name: &str, field_type: PayloadSchemaType) -> Value {
    json!({
        "field_name": field_name,
        "field_type": field_type_to_qdrant(field_type)
    })
}

pub fn build_search_batch_body(searches: Vec<Value>) -> Value {
    json!({
        "searches": searches
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distance_to_qdrant() {
        assert_eq!(distance_to_qdrant(DistanceMetric::Cosine), "Cosine");
        assert_eq!(distance_to_qdrant(DistanceMetric::Euclid), "Euclid");
        assert_eq!(distance_to_qdrant(DistanceMetric::Dot), "Dot");
        assert_eq!(distance_to_qdrant(DistanceMetric::Manhattan), "Manhattan");
    }

    #[test]
    fn test_field_type_to_qdrant() {
        assert_eq!(field_type_to_qdrant(PayloadSchemaType::Keyword), "keyword");
        assert_eq!(field_type_to_qdrant(PayloadSchemaType::Integer), "integer");
        assert_eq!(field_type_to_qdrant(PayloadSchemaType::Float), "float");
        assert_eq!(field_type_to_qdrant(PayloadSchemaType::Text), "text");
        assert_eq!(field_type_to_qdrant(PayloadSchemaType::Bool), "bool");
        assert_eq!(field_type_to_qdrant(PayloadSchemaType::Geo), "geo");
        assert_eq!(
            field_type_to_qdrant(PayloadSchemaType::Datetime),
            "datetime"
        );
    }

    #[test]
    fn test_build_create_collection_body() {
        let body = build_create_collection_body(
            "test",
            384,
            DistanceMetric::Cosine,
            None,
            &None,
            &None,
            None,
            None,
            None,
            None,
        );
        let vectors = body.get("vectors").unwrap();
        assert_eq!(vectors.get("size").unwrap().as_u64(), Some(384));
        assert_eq!(vectors.get("distance").unwrap().as_str(), Some("Cosine"));
    }

    #[test]
    fn test_build_create_collection_body_with_hnsw() {
        let hnsw = HnswConfig::new(32, 200).with_on_disk(true);
        let body = build_create_collection_body(
            "test",
            128,
            DistanceMetric::Dot,
            None,
            &Some(hnsw),
            &None,
            None,
            None,
            None,
            None,
        );
        let vectors = body.get("vectors").unwrap();
        let hnsw_json = vectors.get("hnsw_config").unwrap();
        assert_eq!(hnsw_json.get("m").unwrap().as_u64(), Some(32));
        assert_eq!(hnsw_json.get("ef_construct").unwrap().as_u64(), Some(200));
        assert_eq!(hnsw_json.get("on_disk").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn test_build_create_collection_body_with_shards() {
        let body = build_create_collection_body(
            "test",
            384,
            DistanceMetric::Cosine,
            None,
            &None,
            &None,
            None,
            Some(3),
            None,
            None,
        );
        assert_eq!(body.get("shard_number").unwrap().as_u64(), Some(3));
    }

    #[test]
    fn test_build_create_collection_body_on_disk_payload() {
        let body = build_create_collection_body(
            "test",
            384,
            DistanceMetric::Cosine,
            None,
            &None,
            &None,
            Some(true),
            None,
            None,
            None,
        );
        assert_eq!(body.get("on_disk_payload").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn test_build_create_collection_body_with_replication() {
        let body = build_create_collection_body(
            "test",
            384,
            DistanceMetric::Cosine,
            None,
            &None,
            &None,
            None,
            None,
            Some(2),
            Some(1),
        );
        assert_eq!(body.get("replication_factor").unwrap().as_u64(), Some(2));
        assert_eq!(
            body.get("write_consistency_factor").unwrap().as_u64(),
            Some(1)
        );
    }

    #[test]
    fn test_build_upsert_body() {
        let points = serde_json::json!([{"id": 1, "vector": [1.0, 2.0]}]);
        let body = build_upsert_body(points);
        let pts = body.get("points").unwrap().as_array().unwrap();
        assert_eq!(pts.len(), 1);
        assert_eq!(pts[0].get("id").unwrap().as_u64(), Some(1));
    }

    #[test]
    fn test_build_delete_by_ids_body() {
        let ids = vec![serde_json::json!(1), serde_json::json!("uuid")];
        let body = build_delete_by_ids_body(ids);
        let pts = body.get("points").unwrap().as_array().unwrap();
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn test_build_delete_by_filter_body() {
        let filter = serde_json::json!({"must": [{"key": "color", "match": {"value": "red"}}]});
        let body = build_delete_by_filter_body(filter);
        assert!(body.get("filter").is_some());
    }

    #[test]
    fn test_build_search_body_minimal() {
        let body = build_search_body(vec![1.0, 2.0], 10, None, None, None, None, None, None);
        assert_eq!(body.get("vector").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(body.get("limit").unwrap().as_u64(), Some(10));
        assert_eq!(body.get("with_payload").unwrap().as_bool(), Some(true));
        assert_eq!(body.get("with_vector").unwrap().as_bool(), Some(false));
    }

    #[test]
    fn test_build_search_body_with_options() {
        let filter = serde_json::json!({"must": []});
        let body = build_search_body(
            vec![1.0],
            5,
            Some(2),
            Some(0.5),
            Some(filter),
            Some(false),
            Some(true),
            Some(64),
        );
        assert_eq!(body.get("offset").unwrap().as_u64(), Some(2));
        assert!(
            (body.get("score_threshold").unwrap().as_f64().unwrap() - 0.5).abs() < f64::EPSILON
        );
        assert_eq!(body.get("with_payload").unwrap().as_bool(), Some(false));
        assert_eq!(body.get("with_vector").unwrap().as_bool(), Some(true));
        assert!(body.get("filter").is_some());
        let params = body.get("params").unwrap();
        assert_eq!(params.get("hnsw_ef").unwrap().as_u64(), Some(64));
    }

    #[test]
    fn test_build_get_body() {
        let ids = vec![serde_json::json!(1)];
        let body = build_get_body(ids, Some(false), Some(true));
        assert_eq!(body.get("with_payload").unwrap().as_bool(), Some(false));
        assert_eq!(body.get("with_vector").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn test_build_scroll_body_no_offset() {
        let body = build_scroll_body(100, None, None, None);
        assert_eq!(body.get("limit").unwrap().as_u64(), Some(100));
        assert_eq!(body.get("with_payload").unwrap().as_bool(), Some(true));
        assert!(body.get("offset").is_none());
    }

    #[test]
    fn test_build_scroll_body_with_offset() {
        let body = build_scroll_body(50, Some(serde_json::json!("abc")), Some(false), Some(true));
        assert_eq!(body.get("offset").unwrap().as_str(), Some("abc"));
        assert_eq!(body.get("with_payload").unwrap().as_bool(), Some(false));
        assert_eq!(body.get("with_vector").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn test_build_set_payload_body() {
        let ids = vec![serde_json::json!(1)];
        let payload = serde_json::json!({"color": "red"});
        let body = build_set_payload_body(ids, payload);
        assert_eq!(
            body.get("payload").unwrap().get("color").unwrap().as_str(),
            Some("red")
        );
        assert_eq!(body.get("points").unwrap().as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_build_delete_payload_body() {
        let ids = vec![serde_json::json!(1)];
        let keys = vec!["color".into()];
        let body = build_delete_payload_body(ids, keys);
        let keys_json = body.get("keys").unwrap().as_array().unwrap();
        assert_eq!(keys_json[0].as_str(), Some("color"));
    }

    #[test]
    fn test_build_create_payload_index_body() {
        let body = build_create_payload_index_body("field_x", PayloadSchemaType::Keyword);
        assert_eq!(body.get("field_name").unwrap().as_str(), Some("field_x"));
        assert_eq!(body.get("field_type").unwrap().as_str(), Some("keyword"));
    }

    #[test]
    fn test_build_search_batch_body() {
        let search = build_search_body(vec![1.0], 10, None, None, None, None, None, None);
        let body = build_search_batch_body(vec![search]);
        let searches = body.get("searches").unwrap().as_array().unwrap();
        assert_eq!(searches.len(), 1);
    }

    #[test]
    fn test_build_hnsw_json_all_fields() {
        let hnsw = HnswConfig::new(16, 100)
            .with_full_scan_threshold(5000)
            .with_max_indexing_threads(4)
            .with_on_disk(false)
            .with_payload_m(8);
        let json = super::super::config::build_hnsw_json(&hnsw);
        assert_eq!(json.get("m").unwrap().as_u64(), Some(16));
        assert_eq!(json.get("ef_construct").unwrap().as_u64(), Some(100));
        assert_eq!(
            json.get("full_scan_threshold").unwrap().as_u64(),
            Some(5000)
        );
        assert_eq!(json.get("max_indexing_threads").unwrap().as_u64(), Some(4));
        assert_eq!(json.get("on_disk").unwrap().as_bool(), Some(false));
        assert_eq!(json.get("payload_m").unwrap().as_u64(), Some(8));
    }

    #[test]
    fn test_build_quantization_scalar_json() {
        use crate::types::QuantizationType;
        let qt = QuantizationType::Scalar {
            quantile: Some(0.99),
            always_ram: Some(true),
        };
        let json = super::super::config::build_quantization_json(&qt);
        let scalar = json.get("scalar").expect("scalar key should exist");
        assert_eq!(scalar.get("type").unwrap().as_str(), Some("scalar"));
        assert_eq!(scalar.get("always_ram").unwrap().as_bool(), Some(true));
        let actual = scalar.get("quantile").unwrap().as_f64().unwrap();
        assert!(
            (actual - 0.99).abs() < 0.001,
            "quantile expected ~0.99, got {}",
            actual
        );
    }
}
