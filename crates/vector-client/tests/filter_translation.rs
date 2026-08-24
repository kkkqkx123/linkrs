//! Filter translation consistency between the two Qdrant transports.
//!
//! Every shared [`VectorFilter`] fixture must translate to semantically
//! identical structures on both the gRPC and the HTTP path: same condition
//! types (including typed matches), same fields, same nesting, and a complete
//! `min_should` block.
//!
//! Run with both transports compiled in:
//!
//! ```shell
//! cargo test -p vector-client --features qdrant-http,qdrant-grpc \
//!     --test filter_translation
//! ```

#![cfg(all(feature = "qdrant-http", feature = "qdrant-grpc"))]

use serde_json::{json, Value};
use vector_client::engine::common::filter::{classify_match_any, ClassifiedMatchAny};
use vector_client::engine::grpc::filter::filter_to_proto;
use vector_client::engine::http::filter::convert_filter;
use vector_search::types::*;

// ---- canonical form ------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum CanonicalCondition {
    Match {
        field: String,
        integer: Option<i64>,
        boolean: Option<bool>,
        keyword: Option<String>,
    },
    MatchAnyStrings {
        field: String,
        values: Vec<String>,
    },
    MatchAnyIntegers {
        field: String,
        values: Vec<i64>,
    },
    MatchAnyBooleans {
        field: String,
        values: Vec<bool>,
    },
    Contains {
        field: String,
        value: String,
    },
    Range {
        field: String,
        gt: Option<f64>,
        gte: Option<f64>,
        lt: Option<f64>,
        lte: Option<f64>,
    },
    IsEmpty(String),
    IsNull(String),
    HasId(Vec<String>),
    GeoRadius {
        field: String,
        lat: f64,
        lon: f64,
        radius: f32,
    },
    GeoBoundingBox {
        field: String,
        top_left_lat: f64,
        top_left_lon: f64,
        bottom_right_lat: f64,
        bottom_right_lon: f64,
    },
    ValuesCount {
        field: String,
        gt: Option<u64>,
        gte: Option<u64>,
        lt: Option<u64>,
        lte: Option<u64>,
    },
    Nested {
        field: String,
        filter: Box<CanonicalFilter>,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct CanonicalFilter {
    must: Vec<CanonicalCondition>,
    must_not: Vec<CanonicalCondition>,
    should: Vec<CanonicalCondition>,
    min_should: Option<(Vec<CanonicalCondition>, u64)>,
}

fn match_from_parts(field: &str, value: Value) -> CanonicalCondition {
    if let Some(i) = value.as_i64() {
        CanonicalCondition::Match {
            field: field.to_string(),
            integer: Some(i),
            boolean: None,
            keyword: None,
        }
    } else if let Some(b) = value.as_bool() {
        CanonicalCondition::Match {
            field: field.to_string(),
            integer: None,
            boolean: Some(b),
            keyword: None,
        }
    } else {
        CanonicalCondition::Match {
            field: field.to_string(),
            integer: None,
            boolean: None,
            keyword: Some(value.as_str().expect("string fallback").to_string()),
        }
    }
}

fn range_bounds(obj: &Value) -> (Option<f64>, Option<f64>, Option<f64>, Option<f64>) {
    (
        obj.get("gt").and_then(Value::as_f64),
        obj.get("gte").and_then(Value::as_f64),
        obj.get("lt").and_then(Value::as_f64),
        obj.get("lte").and_then(Value::as_f64),
    )
}

fn count_bounds(obj: &Value) -> (Option<u64>, Option<u64>, Option<u64>, Option<u64>) {
    (
        obj.get("gt").and_then(Value::as_u64),
        obj.get("gte").and_then(Value::as_u64),
        obj.get("lt").and_then(Value::as_u64),
        obj.get("lte").and_then(Value::as_u64),
    )
}

fn canon_condition_from_json(v: &Value) -> CanonicalCondition {
    if let Some(is_empty) = v.get("is_empty") {
        return CanonicalCondition::IsEmpty(
            is_empty
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        );
    }
    if let Some(is_null) = v.get("is_null") {
        return CanonicalCondition::IsNull(
            is_null
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        );
    }
    if let Some(has_id) = v.get("has_id") {
        return CanonicalCondition::HasId(
            has_id
                .as_array()
                .expect("has_id array")
                .iter()
                .map(|id| match id {
                    Value::Number(n) => n.to_string(),
                    Value::String(s) => s.clone(),
                    other => panic!("unexpected has_id element: {}", other),
                })
                .collect(),
        );
    }
    if let Some(nested) = v.get("nested") {
        return CanonicalCondition::Nested {
            field: nested
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            filter: Box::new(canon_filter_from_json(
                nested.get("filter").expect("nested filter"),
            )),
        };
    }

    // Bare-filter conditions (the boolean match_any expansion): the wrapper
    // holds a should-list of singular matches on one field.
    if let Some(bare) = v.get("filter") {
        let should = bare
            .get("should")
            .and_then(Value::as_array)
            .expect("bare filter should list");
        let field = should
            .first()
            .and_then(|c| c.get("key"))
            .and_then(Value::as_str)
            .expect("bare filter field key")
            .to_string();
        let values = should
            .iter()
            .map(|c| {
                c.get("match")
                    .and_then(|m| m.get("value"))
                    .and_then(Value::as_bool)
                    .expect("boolean match value")
            })
            .collect();
        return CanonicalCondition::MatchAnyBooleans { field, values };
    }

    // Field conditions carry a "key".
    let field = v.get("key").and_then(Value::as_str).expect("field key");

    if let Some(m) = v.get("match") {
        if let Some(any) = m.get("any") {
            let items = any.as_array().expect("any list");
            return match classify_match_any(items) {
                ClassifiedMatchAny::Integers(values) => CanonicalCondition::MatchAnyIntegers {
                    field: field.into(),
                    values,
                },
                ClassifiedMatchAny::Strings(values) => CanonicalCondition::MatchAnyStrings {
                    field: field.into(),
                    values,
                },
                // Unreachable from our own translator (pure boolean lists
                // take the bare-filter wrapper above), but kept exhaustive.
                ClassifiedMatchAny::Booleans(values) => CanonicalCondition::MatchAnyBooleans {
                    field: field.into(),
                    values,
                },
            };
        }
        if let Some(text) = m.get("text").and_then(Value::as_str) {
            return CanonicalCondition::Contains {
                field: field.into(),
                value: text.to_string(),
            };
        }
        return match_from_parts(field, m.get("value").cloned().unwrap_or(Value::Null));
    }

    if let Some(range) = v.get("range") {
        let (gt, gte, lt, lte) = range_bounds(range);
        return CanonicalCondition::Range {
            field: field.into(),
            gt,
            gte,
            lt,
            lte,
        };
    }
    if let Some(geo) = v.get("geo_radius") {
        let center = geo.get("center").expect("center");
        return CanonicalCondition::GeoRadius {
            field: field.into(),
            lat: center.get("lat").and_then(Value::as_f64).unwrap_or(0.0),
            lon: center.get("lon").and_then(Value::as_f64).unwrap_or(0.0),
            radius: geo.get("radius").and_then(Value::as_f64).unwrap_or(0.0) as f32,
        };
    }
    if let Some(bbox) = v.get("geo_bounding_box") {
        let tl = bbox.get("top_left").expect("top_left");
        let br = bbox.get("bottom_right").expect("bottom_right");
        return CanonicalCondition::GeoBoundingBox {
            field: field.into(),
            top_left_lat: tl.get("lat").and_then(Value::as_f64).unwrap_or(0.0),
            top_left_lon: tl.get("lon").and_then(Value::as_f64).unwrap_or(0.0),
            bottom_right_lat: br.get("lat").and_then(Value::as_f64).unwrap_or(0.0),
            bottom_right_lon: br.get("lon").and_then(Value::as_f64).unwrap_or(0.0),
        };
    }
    if let Some(count) = v.get("values_count") {
        let (gt, gte, lt, lte) = count_bounds(count);
        return CanonicalCondition::ValuesCount {
            field: field.into(),
            gt,
            gte,
            lt,
            lte,
        };
    }

    panic!("unrecognized JSON condition: {}", v);
}

fn canon_filter_from_json(v: &Value) -> CanonicalFilter {
    let conditions = |key: &str| -> Vec<CanonicalCondition> {
        v.get(key)
            .and_then(Value::as_array)
            .map(|arr| arr.iter().map(canon_condition_from_json).collect())
            .unwrap_or_default()
    };
    let min_should = v.get("min_should").map(|ms| {
        (
            ms.get("conditions")
                .and_then(Value::as_array)
                .map(|arr| arr.iter().map(canon_condition_from_json).collect())
                .unwrap_or_default(),
            ms.get("min_count").and_then(Value::as_u64).unwrap_or(0),
        )
    });
    CanonicalFilter {
        must: conditions("must"),
        must_not: conditions("must_not"),
        should: conditions("should"),
        min_should,
    }
}

// ---- proto canonicalization ----------------------------------------------

use vector_client::engine::grpc::proto;

fn proto_match_value_to_canonical(
    field: &str,
    m: &proto::r#match::MatchValue,
) -> CanonicalCondition {
    use proto::r#match::MatchValue as MV;
    match m {
        MV::Keyword(s) => CanonicalCondition::Match {
            field: field.into(),
            integer: None,
            boolean: None,
            keyword: Some(s.clone()),
        },
        MV::Integer(i) => CanonicalCondition::Match {
            field: field.into(),
            integer: Some(*i),
            boolean: None,
            keyword: None,
        },
        MV::Boolean(b) => CanonicalCondition::Match {
            field: field.into(),
            integer: None,
            boolean: Some(*b),
            keyword: None,
        },
        MV::Text(t) => CanonicalCondition::Contains {
            field: field.into(),
            value: t.clone(),
        },
        MV::Keywords(list) => CanonicalCondition::MatchAnyStrings {
            field: field.into(),
            values: list.strings.clone(),
        },
        MV::Integers(list) => CanonicalCondition::MatchAnyIntegers {
            field: field.into(),
            values: list.integers.clone(),
        },
        other => panic!("unexpected match variant: {:?}", other),
    }
}
fn canon_condition_from_proto(c: &proto::Condition) -> CanonicalCondition {
    use proto::condition::ConditionOneOf as CO;
    let one_of = c.condition_one_of.as_ref().expect("condition oneof");
    match one_of {
        CO::Field(f) => {
            let key = f.key.clone();
            if let Some(m) = &f.r#match {
                let mv = m.match_value.as_ref().expect("match_value");
                return proto_match_value_to_canonical(&key, mv);
            }
            if let Some(range) = &f.range {
                let (gt, gte, lt, lte) = (range.gt, range.gte, range.lt, range.lte);
                return CanonicalCondition::Range {
                    field: key,
                    gt,
                    gte,
                    lt,
                    lte,
                };
            }
            if let Some(geo) = &f.geo_radius {
                let center = geo.center.as_ref().expect("center");
                return CanonicalCondition::GeoRadius {
                    field: key,
                    lat: center.lat,
                    lon: center.lon,
                    radius: geo.radius,
                };
            }
            if let Some(bbox) = &f.geo_bounding_box {
                let tl = bbox.top_left.as_ref().expect("top_left");
                let br = bbox.bottom_right.as_ref().expect("bottom_right");
                return CanonicalCondition::GeoBoundingBox {
                    field: key,
                    top_left_lat: tl.lat,
                    top_left_lon: tl.lon,
                    bottom_right_lat: br.lat,
                    bottom_right_lon: br.lon,
                };
            }
            if let Some(count) = &f.values_count {
                return CanonicalCondition::ValuesCount {
                    field: key,
                    gt: count.gt,
                    gte: count.gte,
                    lt: count.lt,
                    lte: count.lte,
                };
            }
            panic!("unrecognized proto field condition for '{}'", key);
        }
        CO::IsEmpty(cond) => CanonicalCondition::IsEmpty(cond.key.clone()),
        CO::IsNull(cond) => CanonicalCondition::IsNull(cond.key.clone()),
        CO::HasId(cond) => CanonicalCondition::HasId(
            cond.has_id
                .iter()
                .map(|pid| match pid.point_id_options.as_ref() {
                    Some(proto::point_id::PointIdOptions::Num(n)) => n.to_string(),
                    Some(proto::point_id::PointIdOptions::Uuid(u)) => u.clone(),
                    None => String::new(),
                })
                .collect(),
        ),
        CO::Nested(cond) => CanonicalCondition::Nested {
            field: cond.key.clone(),
            filter: Box::new(canon_filter_from_proto(
                cond.filter.as_ref().expect("nested filter"),
            )),
        },
        CO::Filter(bare) => {
            // Bare-filter conditions only arise from the boolean match_any
            // expansion: a should-list of singular boolean matches.
            let field_of = |c: &proto::Condition| match c
                .condition_one_of
                .as_ref()
                .expect("condition oneof")
            {
                CO::Field(f) => f.key.clone(),
                other => panic!("unexpected bare-filter condition: {:?}", other),
            };
            let field = field_of(
                bare.should
                    .first()
                    .expect("bare filter should not be empty"),
            );
            let values = bare
                .should
                .iter()
                .map(
                    |c| match c.condition_one_of.as_ref().expect("condition oneof") {
                        CO::Field(f) => match f
                            .r#match
                            .as_ref()
                            .expect("match")
                            .match_value
                            .as_ref()
                            .expect("match_value")
                        {
                            proto::r#match::MatchValue::Boolean(b) => *b,
                            other => panic!("unexpected bare-filter match variant: {:?}", other),
                        },
                        other => panic!("unexpected bare-filter condition: {:?}", other),
                    },
                )
                .collect();
            CanonicalCondition::MatchAnyBooleans { field, values }
        }
    }
}

fn canon_filter_from_proto(f: &proto::Filter) -> CanonicalFilter {
    let map = |conds: &[proto::Condition]| conds.iter().map(canon_condition_from_proto).collect();
    let min_should = f
        .min_should
        .as_ref()
        .map(|ms| (map(&ms.conditions), ms.min_count));
    CanonicalFilter {
        must: map(&f.must),
        must_not: map(&f.must_not),
        should: map(&f.should),
        min_should,
    }
}

// ---- fixtures -------------------------------------------------------------

fn fixtures() -> Vec<(&'static str, VectorFilter)> {
    vec![
        (
            "keyword match",
            VectorFilter::new().must(FilterCondition::match_value("color", "red")),
        ),
        (
            "integer-shaped match",
            VectorFilter::new().must(FilterCondition::match_value("size", "42")),
        ),
        (
            "boolean-shaped match",
            VectorFilter::new().must(FilterCondition::match_value("active", "true")),
        ),
        (
            "non-canonical integer stays keyword",
            VectorFilter::new().must(FilterCondition::match_value("size", "042")),
        ),
        (
            "match_any strings",
            VectorFilter::new().must(FilterCondition::match_any(
                "tags",
                vec![json!("a"), json!("b")],
            )),
        ),
        (
            "match_any integers",
            VectorFilter::new().must(FilterCondition::match_any(
                "codes",
                vec![json!(1), json!(2), json!(-3)],
            )),
        ),
        (
            "match_any booleans",
            VectorFilter::new().must(FilterCondition::match_any(
                "flags",
                vec![json!(true), json!(false)],
            )),
        ),
        (
            "mixed match_any degrades to strings",
            VectorFilter::new().must(FilterCondition::match_any(
                "misc",
                vec![json!("a"), json!(1)],
            )),
        ),
        (
            "boolean mixed into match_any degrades to strings",
            VectorFilter::new().must(FilterCondition::match_any(
                "misc",
                vec![json!(true), json!("on")],
            )),
        ),
        (
            "range",
            VectorFilter::new().must(FilterCondition::range(
                "price",
                RangeCondition::new().gte(10.0).lt(99.5),
            )),
        ),
        (
            "is_empty",
            VectorFilter::new().must(FilterCondition::is_empty("notes")),
        ),
        (
            "is_null",
            VectorFilter::new().must(FilterCondition::is_null("deleted_at")),
        ),
        (
            "has_id",
            VectorFilter::new().must(FilterCondition::has_id(vec!["1".into(), "uuid-x".into()])),
        ),
        (
            "geo_radius",
            VectorFilter::new().must(FilterCondition::geo_radius(
                "location",
                GeoRadius::new(GeoPoint::new(52.5, 13.4), 1000.0),
            )),
        ),
        (
            "geo bounding box",
            VectorFilter::new().must(FilterCondition::geo_bounding_box(
                "location",
                GeoBoundingBox::new(GeoPoint::new(53.0, 12.0), GeoPoint::new(51.0, 14.0)),
            )),
        ),
        (
            "values_count",
            VectorFilter::new().must(FilterCondition::values_count(
                "tags",
                ValuesCountCondition::new().gt(1).lte(5),
            )),
        ),
        (
            "contains",
            VectorFilter::new().must(FilterCondition::contains("title", "rust")),
        ),
        (
            "nested",
            VectorFilter::new().must(FilterCondition::new(
                "owner",
                ConditionType::Nested {
                    filter: Box::new(
                        VectorFilter::new().must(FilterCondition::match_value("name", "ada")),
                    ),
                },
            )),
        ),
        (
            "min_should",
            VectorFilter {
                must: None,
                must_not: None,
                should: None,
                min_should: Some(MinShouldCondition {
                    conditions: vec![
                        FilterCondition::match_value("a", "x"),
                        FilterCondition::match_value("b", "y"),
                    ],
                    min_count: 2,
                }),
            },
        ),
        (
            "should and min_should coexist",
            VectorFilter {
                must: None,
                must_not: None,
                should: Some(vec![FilterCondition::match_value("tier", "free")]),
                min_should: Some(MinShouldCondition {
                    conditions: vec![
                        FilterCondition::match_value("a", "x"),
                        FilterCondition::match_value("b", "y"),
                    ],
                    min_count: 1,
                }),
            },
        ),
        (
            "combined clauses",
            VectorFilter::new()
                .must(FilterCondition::match_value("kind", "doc"))
                .must_not(FilterCondition::is_null("archived_at"))
                .should(FilterCondition::match_value("tag", "v1")),
        ),
    ]
}

// ---- tests ----------------------------------------------------------------

#[test]
fn grpc_and_http_translations_are_equivalent() {
    for (name, filter) in fixtures() {
        let proto_filter = filter_to_proto(&filter)
            .unwrap_or_else(|e| panic!("{}: grpc translation failed: {}", name, e))
            .unwrap_or_else(|| panic!("{}: grpc translation dropped the filter", name));
        let http_filter = convert_filter(&filter)
            .unwrap_or_else(|e| panic!("{}: http translation failed: {}", name, e))
            .unwrap_or_else(|| panic!("{}: http translation dropped the filter", name));

        let from_proto = canon_filter_from_proto(&proto_filter);
        let from_json = canon_filter_from_json(&http_filter);

        assert_eq!(
            from_proto, from_json,
            "fixture '{}' diverges between transports:\n  grpc: {:?}\n  http: {:?}",
            name, from_proto, from_json
        );

        // The degraded mixed-type case must keep every element.
        if name == "mixed match_any degrades to strings" {
            assert_eq!(
                from_proto.must,
                vec![CanonicalCondition::MatchAnyStrings {
                    field: "misc".into(),
                    values: vec!["a".to_string(), "1".to_string()],
                }],
                "mixed match_any lost elements"
            );
        }
    }
}

#[test]
fn empty_nested_filters_fail_loudly_on_both_transports() {
    let filter = VectorFilter::new().must(FilterCondition::new(
        "owner",
        ConditionType::Nested {
            filter: Box::new(VectorFilter::new()),
        },
    ));

    assert!(
        filter_to_proto(&filter).is_err(),
        "grpc must reject an empty nested filter"
    );
    assert!(
        convert_filter(&filter).is_err(),
        "http must reject an empty nested filter"
    );
}
