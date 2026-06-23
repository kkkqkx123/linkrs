use tonic::Status;

use crate::error::VectorClientError;
use crate::types::*;

use super::super::common::convert::extract_search_params;

impl From<Status> for VectorClientError {
    fn from(status: Status) -> Self {
        match status.code() {
            tonic::Code::NotFound => {
                let msg = status.message().to_string();
                if msg.contains("collection") || msg.contains("Collection") {
                    VectorClientError::CollectionNotFound(msg)
                } else {
                    VectorClientError::PointNotFound(msg, String::new())
                }
            }
            tonic::Code::AlreadyExists => {
                VectorClientError::CollectionAlreadyExists(status.message().to_string())
            }
            tonic::Code::InvalidArgument => {
                VectorClientError::InternalError(status.message().to_string())
            }
            tonic::Code::Unavailable | tonic::Code::DeadlineExceeded => {
                VectorClientError::ConnectionFailed(status.message().to_string())
            }
            _ => VectorClientError::QdrantGrpcError(status.to_string()),
        }
    }
}

use super::proto;

pub fn point_id_to_proto(id: &str) -> proto::PointId {
    if let Ok(num) = id.parse::<u64>() {
        proto::PointId {
            point_id_options: Some(proto::point_id::PointIdOptions::Num(num)),
        }
    } else {
        proto::PointId {
            point_id_options: Some(proto::point_id::PointIdOptions::Uuid(id.to_string())),
        }
    }
}

pub fn point_struct_to_proto(point: &VectorPoint) -> proto::PointStruct {
    let id = point_id_to_proto(&point.id.to_string());

    let vectors = proto::Vectors {
        vectors_options: Some(proto::vectors::VectorsOptions::Vector(proto::Vector {
            data: point.vector.clone(),
            indices: None,
            vectors_count: None,
        })),
    };

    let payload = point
        .payload
        .as_ref()
        .map(payload_to_proto_map)
        .unwrap_or_default();

    proto::PointStruct {
        id: Some(id),
        payload,
        vectors: Some(vectors),
    }
}

pub fn payload_to_proto_map(payload: &Payload) -> std::collections::HashMap<String, proto::Value> {
    payload
        .iter()
        .map(|(k, v)| (k.clone(), json_value_to_proto_value(v)))
        .collect()
}

fn json_value_to_proto_value(v: &serde_json::Value) -> proto::Value {
    match v {
        serde_json::Value::Null => proto::Value {
            kind: Some(proto::value::Kind::NullValue(0)),
        },
        serde_json::Value::Bool(b) => proto::Value {
            kind: Some(proto::value::Kind::BoolValue(*b)),
        },
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                proto::Value {
                    kind: Some(proto::value::Kind::IntegerValue(i)),
                }
            } else if let Some(f) = n.as_f64() {
                proto::Value {
                    kind: Some(proto::value::Kind::DoubleValue(f)),
                }
            } else {
                proto::Value {
                    kind: Some(proto::value::Kind::NullValue(0)),
                }
            }
        }
        serde_json::Value::String(s) => proto::Value {
            kind: Some(proto::value::Kind::StringValue(s.clone())),
        },
        serde_json::Value::Array(arr) => {
            let values: Vec<proto::Value> = arr.iter().map(json_value_to_proto_value).collect();
            proto::Value {
                kind: Some(proto::value::Kind::ListValue(proto::ListValue { values })),
            }
        }
        serde_json::Value::Object(obj) => {
            let fields: std::collections::HashMap<String, proto::Value> = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_value_to_proto_value(v)))
                .collect();
            proto::Value {
                kind: Some(proto::value::Kind::StructValue(proto::Struct { fields })),
            }
        }
    }
}

fn proto_value_to_json_value(v: &proto::Value) -> serde_json::Value {
    match &v.kind {
        Some(proto::value::Kind::NullValue(_)) => serde_json::Value::Null,
        Some(proto::value::Kind::DoubleValue(f)) => {
            if f.is_finite() {
                serde_json::Value::Number(
                    serde_json::Number::from_f64(*f).expect("finite f64 should convert"),
                )
            } else {
                serde_json::Value::Null
            }
        }
        Some(proto::value::Kind::IntegerValue(i)) => {
            serde_json::Value::Number(serde_json::Number::from(*i))
        }
        Some(proto::value::Kind::StringValue(s)) => serde_json::Value::String(s.clone()),
        Some(proto::value::Kind::BoolValue(b)) => serde_json::Value::Bool(*b),
        Some(proto::value::Kind::StructValue(s)) => {
            let mut map = serde_json::Map::new();
            for (k, v) in &s.fields {
                map.insert(k.clone(), proto_value_to_json_value(v));
            }
            serde_json::Value::Object(map)
        }
        Some(proto::value::Kind::ListValue(l)) => {
            let arr: Vec<serde_json::Value> =
                l.values.iter().map(proto_value_to_json_value).collect();
            serde_json::Value::Array(arr)
        }
        None => serde_json::Value::Null,
    }
}

pub fn distance_to_proto(distance: DistanceMetric) -> proto::Distance {
    match distance {
        DistanceMetric::Cosine => proto::Distance::Cosine,
        DistanceMetric::Euclid => proto::Distance::Euclid,
        DistanceMetric::Dot => proto::Distance::Dot,
        DistanceMetric::Manhattan => proto::Distance::Manhattan,
    }
}

pub fn collection_config_to_create(name: &str, cfg: &CollectionConfig) -> proto::CreateCollection {
    let distance = distance_to_proto(cfg.distance);

    let hnsw_config: Option<proto::HnswConfigDiff> =
        cfg.hnsw_config.as_ref().map(|hnsw| proto::HnswConfigDiff {
            m: Some(hnsw.m as u64),
            ef_construct: Some(hnsw.ef_construct as u64),
            full_scan_threshold: hnsw.full_scan_threshold.map(|v| v as u64),
            max_indexing_threads: hnsw.max_indexing_threads.map(|v| v as u64),
            on_disk: hnsw.on_disk,
            payload_m: hnsw.payload_m.map(|v| v as u64),
        });

    let quantization_config: Option<proto::QuantizationConfig> =
        cfg.quantization_config.as_ref().and_then(|qc| {
            if !qc.enabled {
                return None;
            }
            qc.quant_type.as_ref().map(|qt| match qt {
                QuantizationType::Scalar {
                    quantile,
                    always_ram,
                } => proto::QuantizationConfig {
                    quantization: Some(proto::quantization_config::Quantization::Scalar(
                        proto::ScalarQuantization {
                            r#type: proto::QuantizationType::Int8 as i32,
                            quantile: *quantile,
                            always_ram: *always_ram,
                        },
                    )),
                },
                QuantizationType::Product {
                    compression,
                    always_ram,
                } => {
                    let ratio = match compression {
                        CompressionRatio::X4 => proto::CompressionRatio::X4,
                        CompressionRatio::X8 => proto::CompressionRatio::X8,
                        CompressionRatio::X16 => proto::CompressionRatio::X16,
                        CompressionRatio::X32 => proto::CompressionRatio::X32,
                        CompressionRatio::X64 => proto::CompressionRatio::X64,
                    };
                    proto::QuantizationConfig {
                        quantization: Some(proto::quantization_config::Quantization::Product(
                            proto::ProductQuantization {
                                compression: ratio as i32,
                                always_ram: *always_ram,
                            },
                        )),
                    }
                }
                QuantizationType::Binary { always_ram } => proto::QuantizationConfig {
                    quantization: Some(proto::quantization_config::Quantization::Binary(
                        proto::BinaryQuantization {
                            always_ram: *always_ram,
                        },
                    )),
                },
            })
        });

    let vectors_config = proto::VectorsConfig {
        config: Some(proto::vectors_config::Config::Params(proto::VectorParams {
            size: cfg.vector_size as u64,
            distance: distance as i32,
            hnsw_config,
            quantization_config,
            on_disk: None,
            datatype: None,
            multivector_config: None,
        })),
    };

    proto::CreateCollection {
        collection_name: name.to_string(),
        hnsw_config,
        wal_config: None,
        optimizers_config: None,
        shard_number: cfg.shard_number.map(|v| v as u32),
        on_disk_payload: cfg.on_disk_payload,
        timeout: None,
        vectors_config: Some(vectors_config),
        replication_factor: cfg.replication_factor.map(|v| v as u32),
        write_consistency_factor: cfg.write_consistency_factor.map(|v| v as u32),
        init_from_collection: None,
        quantization_config,
        sharding_method: None,
        sparse_vectors_config: None,
        strict_mode_config: None,
    }
}

pub fn upsert_result_from_proto(result: proto::UpdateResult) -> UpsertResult {
    UpsertResult {
        operation_id: result.operation_id,
        status: if result.status() == proto::UpdateStatus::Completed {
            UpsertStatus::Completed
        } else {
            UpsertStatus::Acknowledged
        },
    }
}

fn extract_point_core(
    id: &Option<proto::PointId>,
    payload: &std::collections::HashMap<String, proto::Value>,
    vectors: &Option<proto::Vectors>,
) -> (String, Option<Payload>, Option<Vec<f32>>) {
    let id = id.as_ref().map(point_id_to_string).unwrap_or_default();

    let payload = if payload.is_empty() {
        None
    } else {
        let mut map = Payload::new();
        for (k, v) in payload {
            map.insert(k.clone(), proto_value_to_json_value(v));
        }
        Some(map)
    };

    let vector = vectors.as_ref().and_then(|v| {
        v.vectors_options.as_ref().and_then(|opts| match opts {
            proto::vectors::VectorsOptions::Vector(vec) => Some(vec.data.clone()),
            _ => None,
        })
    });

    (id, payload, vector)
}

pub fn scored_point_from_proto(point: proto::ScoredPoint) -> SearchResult {
    let (id, payload, vector) = extract_point_core(&point.id, &point.payload, &point.vectors);
    SearchResult {
        id: id.into(),
        score: point.score,
        payload,
        vector,
    }
}

pub fn retrieved_point_from_proto(point: proto::RetrievedPoint) -> VectorPoint {
    let (id, payload, vector) = extract_point_core(&point.id, &point.payload, &point.vectors);
    VectorPoint {
        id: id.into(),
        vector: vector.unwrap_or_default(),
        payload,
    }
}

fn point_id_to_string(id: &proto::PointId) -> String {
    match &id.point_id_options {
        Some(proto::point_id::PointIdOptions::Num(n)) => n.to_string(),
        Some(proto::point_id::PointIdOptions::Uuid(u)) => u.clone(),
        None => String::new(),
    }
}

pub fn search_query_to_proto(collection: &str, query: &SearchQuery) -> proto::SearchPoints {
    let filter = query
        .filter
        .as_ref()
        .and_then(|f| super::filter::filter_to_proto(f).ok())
        .flatten();

    let extracted = extract_search_params(query);

    let params = proto::SearchParams {
        hnsw_ef: extracted.hnsw_ef.map(|v| v as u64),
        exact: None,
        quantization: None,
        indexed_only: None,
    };

    proto::SearchPoints {
        collection_name: collection.to_string(),
        vector: query.vector.clone(),
        filter,
        limit: extracted.limit as u64,
        with_payload: Some(proto::WithPayloadSelector {
            selector_options: Some(proto::with_payload_selector::SelectorOptions::Enable(
                query.with_payload.unwrap_or(true),
            )),
        }),
        params: Some(params),
        score_threshold: query.score_threshold,
        offset: query.offset.map(|v| v as u64),
        vector_name: None,
        with_vectors: Some(proto::WithVectorsSelector {
            selector_options: Some(proto::with_vectors_selector::SelectorOptions::Enable(
                query.with_vector.unwrap_or(false),
            )),
        }),
        read_consistency: None,
        timeout: None,
        shard_key_selector: None,
        sparse_indices: None,
    }
}

fn distance_from_proto(d: i32) -> DistanceMetric {
    match d {
        1 => DistanceMetric::Cosine,
        2 => DistanceMetric::Euclid,
        3 => DistanceMetric::Dot,
        4 => DistanceMetric::Manhattan,
        _ => DistanceMetric::Cosine,
    }
}

fn extract_vector_config(cfg: &proto::CollectionConfig) -> (usize, DistanceMetric) {
    cfg.params
        .as_ref()
        .and_then(|params| params.vectors_config.as_ref())
        .and_then(|vc| vc.config.as_ref())
        .and_then(|c| match c {
            proto::vectors_config::Config::Params(params) => {
                Some((params.size as usize, distance_from_proto(params.distance)))
            }
            _ => None,
        })
        .unwrap_or((1536, DistanceMetric::Cosine))
}

pub fn collection_info_from_proto(
    info: proto::CollectionInfo,
    name: &str,
) -> Result<CollectionInfo, VectorClientError> {
    let status = match info.status() {
        proto::CollectionStatus::Green => CollectionStatus::Green,
        proto::CollectionStatus::Yellow => CollectionStatus::Yellow,
        proto::CollectionStatus::Red => CollectionStatus::Red,
        _ => CollectionStatus::Grey,
    };

    let (vector_size, distance) = info
        .config
        .as_ref()
        .map(extract_vector_config)
        .unwrap_or((1536, DistanceMetric::Cosine));

    let params = info.config.as_ref().and_then(|c| c.params.as_ref());
    let config = CollectionConfig {
        vector_size,
        distance,
        index_type: None,
        hnsw_config: None,
        quantization_config: None,
        replication_factor: params.and_then(|p| p.replication_factor.map(|v| v as usize)),
        write_consistency_factor: params
            .and_then(|p| p.write_consistency_factor.map(|v| v as usize)),
        on_disk_payload: params.map(|p| p.on_disk_payload),
        shard_number: params.map(|p| p.shard_number as usize),
    };

    Ok(CollectionInfo {
        name: name.to_string(),
        vector_count: info.vectors_count.unwrap_or(0),
        indexed_vector_count: info.indexed_vectors_count.unwrap_or(0),
        points_count: info.points_count.unwrap_or(0),
        segments_count: info.segments_count,
        config,
        status,
    })
}

pub fn payload_schema_type_to_field_type(schema: PayloadSchemaType) -> proto::FieldType {
    match schema {
        PayloadSchemaType::Keyword => proto::FieldType::Keyword,
        PayloadSchemaType::Integer => proto::FieldType::Integer,
        PayloadSchemaType::Float => proto::FieldType::Float,
        PayloadSchemaType::Text => proto::FieldType::Text,
        PayloadSchemaType::Bool => proto::FieldType::Bool,
        PayloadSchemaType::Geo => proto::FieldType::Geo,
        PayloadSchemaType::Datetime => proto::FieldType::Datetime,
    }
}

pub fn payload_schema_type_from_proto(data_type: i32) -> PayloadSchemaType {
    let dt = proto::PayloadSchemaType::try_from(data_type)
        .unwrap_or(proto::PayloadSchemaType::UnknownType);
    match dt {
        proto::PayloadSchemaType::Keyword => PayloadSchemaType::Keyword,
        proto::PayloadSchemaType::Integer => PayloadSchemaType::Integer,
        proto::PayloadSchemaType::Float => PayloadSchemaType::Float,
        proto::PayloadSchemaType::Text => PayloadSchemaType::Text,
        proto::PayloadSchemaType::Bool => PayloadSchemaType::Bool,
        proto::PayloadSchemaType::Geo => PayloadSchemaType::Geo,
        proto::PayloadSchemaType::Datetime => PayloadSchemaType::Datetime,
        _ => PayloadSchemaType::Keyword,
    }
}
