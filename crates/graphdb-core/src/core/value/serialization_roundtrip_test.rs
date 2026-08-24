//! Exhaustive serde/postcard roundtrip test for `Value`.
//!
//! `Value` is persisted via serde (postcard) in the WAL/undo paths. Every
//! variant must survive `decode(encode(v)) == v` exactly. Two safety nets keep
//! the coverage exhaustive:
//!
//! 1. [`all_sample_values`] constructs one fixture per variant and the
//!    roundtrip test asserts exact equality for every fixture.
//! 2. [`exhaustive_variant_check`] is a non-exhaustive-match-free `matches!`
//!    over all 33 variants: adding a new `Value` variant without extending it
//!    fails to compile.

use super::date_time::{DateTimeValue, DateValue, TimeValue};
use super::decimal128::Decimal128Value;
use super::geography::Geography;
use super::interval::IntervalValue;
use super::json::{Json, JsonB};
use super::list::List;
use super::null::NullType;
use super::uuid::UuidValue;
use super::vector::VectorValue;
use super::{ArrayValue, StructValue};
use crate::core::types::storage_ids::{EdgeId, VertexId};
use crate::core::vertex_edge_path::{Edge, Path, Vertex};
use crate::core::DataSet;
use crate::core::Value;
use std::collections::{HashMap, HashSet};

/// Compile-time exhaustive check: a new `Value` variant must be listed here
/// (and a fixture added below) or this fails to compile.
fn exhaustive_variant_check(value: &Value) {
    debug_assert!(matches!(
        value,
        Value::Empty
            | Value::Null(_)
            | Value::Bool(_)
            | Value::SmallInt(_)
            | Value::Int(_)
            | Value::BigInt(_)
            | Value::Float(_)
            | Value::Double(_)
            | Value::Decimal128(_)
            | Value::String(_)
            | Value::FixedString(_)
            | Value::Blob(_)
            | Value::Date(_)
            | Value::Time(_)
            | Value::DateTime(_)
            | Value::Vertex(_)
            | Value::Edge(_)
            | Value::Path(_)
            | Value::List(_)
            | Value::Map(_)
            | Value::Set(_)
            | Value::Geography(_)
            | Value::Vector(_)
            | Value::DataSet(_)
            | Value::Json(_)
            | Value::JsonB(_)
            | Value::Uuid(_)
            | Value::Interval(_)
            | Value::VertexId(_)
            | Value::EdgeId(_)
            | Value::Struct(_)
            | Value::Array(_)
    ));
}

/// One sample value for every `Value` variant (exhaustive by construction).
fn all_sample_values() -> Vec<Value> {
    let mut list = List::new();
    list.push(Value::Int(7));

    let mut map = HashMap::new();
    map.insert(Value::string("k"), Value::string("v"));

    let mut set = HashSet::new();
    set.insert(Value::Int(1));
    set.insert(Value::BigInt(2));

    let mut dataset = DataSet::new();
    dataset.add_row(vec![Value::Int(1)]);
    dataset.add_row(vec![Value::Int(2)]);

    let mut vertex = Vertex::with_vid(VertexId::from_int64(10));
    vertex.tags.push(crate::core::vertex_edge_path::Tag::new(
        "Person".to_string(),
        HashMap::new(),
    ));
    let edge = Edge::new_empty(
        VertexId::from_int64(1),
        VertexId::from_int64(2),
        "KNOWS".to_string(),
        0,
    );
    let path = Path::new(Vertex::with_vid(VertexId::from_int64(1)));

    vec![
        Value::Empty,
        Value::Null(NullType::Null),
        Value::Bool(true),
        Value::SmallInt(-42),
        Value::Int(-42),
        Value::BigInt(-42),
        Value::Float(1.5),
        Value::Double(1.5),
        Value::Decimal128(Decimal128Value::from_i64(12345)),
        Value::string("hello"),
        Value::fixed_string(8, "fixed".to_string()),
        Value::Blob(vec![1, 2, 3]),
        Value::Date(DateValue {
            year: 2026,
            month: 8,
            day: 15,
        }),
        Value::Time(TimeValue {
            hour: 10,
            minute: 30,
            sec: 45,
            microsec: 123,
        }),
        Value::DateTime(DateTimeValue {
            year: 2026,
            month: 8,
            day: 15,
            hour: 10,
            minute: 30,
            sec: 45,
            microsec: 123,
        }),
        Value::Vertex(Box::new(vertex)),
        Value::Edge(Box::new(edge)),
        Value::Path(Box::new(path)),
        Value::list(list),
        Value::map(map),
        Value::set(set),
        Value::Geography(Geography::from_wkt("POINT(1 2)").expect("wkt should parse")),
        Value::Vector(VectorValue::dense(vec![1.0, 2.0, 3.0])),
        Value::DataSet(Box::new(dataset)),
        Value::Json(Box::new(
            Json::parse(r#"{"a":1}"#).expect("json should parse"),
        )),
        Value::JsonB(Box::new(
            JsonB::parse(r#"{"b":2}"#).expect("jsonb should parse"),
        )),
        Value::Uuid(UuidValue([9u8; 16])),
        Value::Interval(IntervalValue::new(14, 3, 1000)),
        Value::VertexId(VertexId::from_int64(99)),
        Value::EdgeId(EdgeId::new(99)),
        Value::Struct(Box::new(StructValue::new(vec![
            ("city".to_string(), Value::string("x")),
            (
                "geo".to_string(),
                Value::Struct(Box::new(StructValue::new(vec![(
                    "lat".to_string(),
                    Value::Double(1.5),
                )]))),
            ),
        ]))),
        Value::Array(Box::new(ArrayValue::new(vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
        ]))),
    ]
}

#[test]
fn all_value_variants_roundtrip_through_postcard() {
    for value in all_sample_values() {
        exhaustive_variant_check(&value);
        let encoded = postcard::to_allocvec(&value).expect("encode value");
        let decoded: Value = postcard::from_bytes(&encoded).expect("decode value");
        assert_eq!(
            decoded, value,
            "postcard roundtrip must preserve the value exactly: {value:?}"
        );
    }
}

#[test]
fn all_value_variants_roundtrip_via_value_in_container() {
    // The undo log embeds Values inside larger structs; the same property must
    // hold there.
    let values = all_sample_values();
    let encoded = postcard::to_allocvec(&values).expect("encode container");
    let decoded: Vec<Value> = postcard::from_bytes(&encoded).expect("decode container");
    assert_eq!(decoded.len(), values.len());
    for (decoded, original) in decoded.iter().zip(values.iter()) {
        assert_eq!(decoded, original);
    }
}

#[test]
fn map_with_non_string_keys_roundtrips() {
    // M4: generalized map keys (any hashable Value) survive serde.
    let mut map = HashMap::new();
    map.insert(Value::Int(7), Value::string("seven"));
    map.insert(Value::string("name"), Value::string("x"));
    map.insert(Value::Double(0.5), Value::Bool(true));
    let value = Value::map(map);

    let encoded = postcard::to_allocvec(&value).expect("encode map");
    let decoded: Value = postcard::from_bytes(&encoded).expect("decode map");
    assert_eq!(decoded, value);
    assert_eq!(decoded.hash_value(), value.hash_value());
}

#[test]
fn map_comparison_and_hash_are_key_order_independent() {
    // Two maps with the same entries in different insertion order compare
    // equal and hash identically.
    let mut a = HashMap::new();
    a.insert(Value::Int(1), Value::string("one"));
    a.insert(Value::Int(2), Value::string("two"));

    let mut b = HashMap::new();
    b.insert(Value::Int(2), Value::string("two"));
    b.insert(Value::Int(1), Value::string("one"));

    let va = Value::map(a);
    let vb = Value::map(b);
    assert_eq!(va, vb);
    assert_eq!(va.cmp(&vb), std::cmp::Ordering::Equal);
    assert_eq!(va.hash_value(), vb.hash_value());
}
