use crate::error::VectorClientError;
use crate::types::*;

use super::super::common::filter::{process_filter, ConditionHandler};
use super::proto;

pub fn filter_to_proto(filter: &VectorFilter) -> Result<Option<proto::Filter>, VectorClientError> {
    let handler = ProtoConditionHandler;
    process_filter(filter, &handler)
}

struct ProtoConditionHandler;

fn point_id_to_proto(id: &str) -> proto::PointId {
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

fn field_condition(
    key: String,
    r#match: Option<proto::Match>,
    range: Option<proto::Range>,
    geo_bounding_box: Option<proto::GeoBoundingBox>,
    geo_radius: Option<proto::GeoRadius>,
    values_count: Option<proto::ValuesCount>,
) -> proto::Condition {
    proto::Condition {
        condition_one_of: Some(proto::condition::ConditionOneOf::Field(
            proto::FieldCondition {
                key,
                r#match,
                range,
                geo_bounding_box,
                geo_radius,
                values_count,
                geo_polygon: None,
                datetime_range: None,
            },
        )),
    }
}

impl ConditionHandler for ProtoConditionHandler {
    type Condition = proto::Condition;
    type Filter = proto::Filter;

    fn handle_match(&self, field: &str, value: &str) -> proto::Condition {
        field_condition(
            field.to_string(),
            Some(proto::Match {
                match_value: Some(proto::r#match::MatchValue::Keyword(value.to_string())),
            }),
            None,
            None,
            None,
            None,
        )
    }

    fn handle_match_any(&self, field: &str, values: &[serde_json::Value]) -> proto::Condition {
        let keywords: Vec<String> = values
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();
        field_condition(
            field.to_string(),
            Some(proto::Match {
                match_value: Some(proto::r#match::MatchValue::Keywords(
                    proto::RepeatedStrings { strings: keywords },
                )),
            }),
            None,
            None,
            None,
            None,
        )
    }

    fn handle_range(&self, field: &str, range: &RangeCondition) -> proto::Condition {
        field_condition(
            field.to_string(),
            None,
            Some(proto::Range {
                lt: range.lt,
                gt: range.gt,
                gte: range.gte,
                lte: range.lte,
            }),
            None,
            None,
            None,
        )
    }

    fn handle_is_empty(&self, field: &str) -> proto::Condition {
        proto::Condition {
            condition_one_of: Some(proto::condition::ConditionOneOf::IsEmpty(
                proto::IsEmptyCondition {
                    key: field.to_string(),
                },
            )),
        }
    }

    fn handle_is_null(&self, field: &str) -> proto::Condition {
        proto::Condition {
            condition_one_of: Some(proto::condition::ConditionOneOf::IsNull(
                proto::IsNullCondition {
                    key: field.to_string(),
                },
            )),
        }
    }

    fn handle_has_id(&self, ids: &[String]) -> proto::Condition {
        let proto_ids: Vec<proto::PointId> = ids.iter().map(|id| point_id_to_proto(id)).collect();
        proto::Condition {
            condition_one_of: Some(proto::condition::ConditionOneOf::HasId(
                proto::HasIdCondition { has_id: proto_ids },
            )),
        }
    }

    fn handle_geo_radius(&self, field: &str, radius: &GeoRadius) -> proto::Condition {
        field_condition(
            field.to_string(),
            None,
            None,
            None,
            Some(proto::GeoRadius {
                center: Some(proto::GeoPoint {
                    lon: radius.center.lon,
                    lat: radius.center.lat,
                }),
                radius: radius.radius as f32,
            }),
            None,
        )
    }

    fn handle_geo_bounding_box(&self, field: &str, bbox: &GeoBoundingBox) -> proto::Condition {
        field_condition(
            field.to_string(),
            None,
            None,
            Some(proto::GeoBoundingBox {
                top_left: Some(proto::GeoPoint {
                    lon: bbox.top_left.lon,
                    lat: bbox.top_left.lat,
                }),
                bottom_right: Some(proto::GeoPoint {
                    lon: bbox.bottom_right.lon,
                    lat: bbox.bottom_right.lat,
                }),
            }),
            None,
            None,
        )
    }

    fn handle_values_count(&self, field: &str, count: &ValuesCountCondition) -> proto::Condition {
        field_condition(
            field.to_string(),
            None,
            None,
            None,
            None,
            Some(proto::ValuesCount {
                lt: count.lt,
                gt: count.gt,
                gte: count.gte,
                lte: count.lte,
            }),
        )
    }

    fn handle_contains(&self, field: &str, value: &str) -> proto::Condition {
        field_condition(
            field.to_string(),
            Some(proto::Match {
                match_value: Some(proto::r#match::MatchValue::Text(value.to_string())),
            }),
            None,
            None,
            None,
            None,
        )
    }

    fn handle_nested(&self, field: &str, filter: proto::Filter) -> proto::Condition {
        proto::Condition {
            condition_one_of: Some(proto::condition::ConditionOneOf::Nested(
                proto::NestedCondition {
                    key: field.to_string(),
                    filter: Some(filter),
                },
            )),
        }
    }

    fn build_filter(
        &self,
        must: Vec<proto::Condition>,
        must_not: Vec<proto::Condition>,
        should: Vec<proto::Condition>,
        min_should: Option<(Vec<proto::Condition>, usize)>,
    ) -> Option<proto::Filter> {
        let min_should = min_should.map(|(conditions, min_count)| proto::MinShould {
            conditions,
            min_count: min_count as u64,
        });

        Some(proto::Filter {
            should,
            must,
            must_not,
            min_should,
        })
    }
}
