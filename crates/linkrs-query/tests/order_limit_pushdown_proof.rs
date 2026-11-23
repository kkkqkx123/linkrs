//! Proof input for a future order-limit pushdown.
//!
//! An ordered index may answer `ORDER BY key LIMIT n` only when its byte order
//! is the same order the executor uses for the sort key. This file compares
//! both orders for every scalar kind the index codec supports and records the
//! shapes that must stay unpushed.

use std::cmp::Ordering;

use linkrs_core::value::date_time::{DateTimeValue, DateValue, TimeValue};
use linkrs_core::value::decimal128::Decimal128Value;
use linkrs_core::value::ordered_codec::OrderedCodec;
use linkrs_core::value::UuidValue;
use linkrs_core::{NullType, Value};
use linkrs_query::executor::streaming::helpers::compare_values;

fn codec_matches_sort(values: &[Value], null_last: bool) -> bool {
    let codec = OrderedCodec::new();
    let mut encoded = Vec::with_capacity(values.len());
    for value in values {
        let bytes = if null_last {
            codec.encode_null_last(value)
        } else {
            codec.encode(value)
        };
        let Ok(bytes) = bytes else {
            return false;
        };
        encoded.push(bytes);
    }
    for (i, a) in values.iter().enumerate() {
        for (j, b) in values.iter().enumerate() {
            if compare_values(a, b) != encoded[i].cmp(&encoded[j]) {
                return false;
            }
        }
    }
    true
}

fn assert_compatible(label: &str, values: &[Value]) {
    assert!(
        codec_matches_sort(values, false),
        "index order must match sort order for {label}"
    );
}

#[test]
fn scalar_kinds_with_proven_index_order() {
    assert_compatible("bool", &[Value::Bool(false), Value::Bool(true)]);
    assert_compatible(
        "smallint",
        &[Value::SmallInt(-2), Value::SmallInt(0), Value::SmallInt(5)],
    );
    assert_compatible("int", &[Value::Int(-100), Value::Int(0), Value::Int(100)]);
    assert_compatible(
        "bigint",
        &[
            Value::BigInt(-(1i64 << 40)),
            Value::BigInt(0),
            Value::BigInt(1i64 << 40),
        ],
    );
    assert_compatible(
        "float",
        &[Value::Float(-1.5), Value::Float(0.0), Value::Float(2.5)],
    );
    assert_compatible(
        "double",
        &[Value::Double(-1.5), Value::Double(0.0), Value::Double(2.5)],
    );
    assert_compatible(
        "string",
        &[Value::string("a"), Value::string("aa"), Value::string("b")],
    );
    assert_compatible(
        "fixed_string",
        &[
            Value::fixed_string(2, "a".to_string()),
            Value::fixed_string(2, "aa".to_string()),
            Value::fixed_string(2, "b".to_string()),
        ],
    );
    assert_compatible(
        "date",
        &[
            Value::Date(DateValue {
                year: 2024,
                month: 1,
                day: 2,
            }),
            Value::Date(DateValue {
                year: 2024,
                month: 1,
                day: 10,
            }),
            Value::Date(DateValue {
                year: 2025,
                month: 1,
                day: 1,
            }),
        ],
    );
    assert_compatible(
        "time",
        &[
            Value::Time(TimeValue {
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            }),
            Value::Time(TimeValue {
                hour: 1,
                minute: 2,
                sec: 3,
                microsec: 0,
            }),
            Value::Time(TimeValue {
                hour: 23,
                minute: 59,
                sec: 59,
                microsec: 999_999,
            }),
        ],
    );
    assert_compatible(
        "datetime",
        &[
            Value::DateTime(DateTimeValue {
                year: 2024,
                month: 1,
                day: 2,
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            }),
            Value::DateTime(DateTimeValue {
                year: 2024,
                month: 1,
                day: 10,
                hour: 0,
                minute: 0,
                sec: 0,
                microsec: 0,
            }),
        ],
    );
    assert_compatible(
        "decimal",
        &[
            Value::Decimal128(Decimal128Value::from_i64(-1)),
            Value::Decimal128(Decimal128Value::from_i64(0)),
            Value::Decimal128(Decimal128Value::from_i64(1)),
        ],
    );
    assert_compatible(
        "uuid",
        &[
            Value::Uuid(UuidValue([0u8; 16])),
            Value::Uuid(UuidValue([1u8; 16])),
        ],
    );
}

#[test]
fn null_placement_must_use_null_last_encoding() {
    let values = [
        Value::BigInt(1),
        Value::BigInt(2),
        Value::Null(NullType::Null),
    ];
    assert!(!codec_matches_sort(&values, false));
    assert!(codec_matches_sort(&values, true));
}

#[test]
fn nan_byte_order_matches_the_total_order_but_planner_still_rejects_nan() {
    let values = [
        Value::Double(1.0),
        Value::Double(f64::NAN),
        Value::Double(2.0),
    ];
    assert!(codec_matches_sort(&values, false));
    assert_eq!(compare_values(&values[1], &values[0]), Ordering::Less);
}

#[test]
fn date_wide_year_ranges_are_order_compatible() {
    let values = [
        Value::Date(DateValue {
            year: 999,
            month: 1,
            day: 1,
        }),
        Value::Date(DateValue {
            year: 1000,
            month: 1,
            day: 1,
        }),
    ];
    assert!(codec_matches_sort(&values, false));
}

#[test]
fn mixed_numeric_kinds_are_not_order_compatible() {
    let values = [Value::BigInt(-100), Value::Int(5)];
    assert!(!codec_matches_sort(&values, false));
    assert_eq!(compare_values(&values[0], &values[1]), Ordering::Less);
    assert_eq!(
        compare_values(&values[0], &values[1]),
        linkrs_core::Value::cmp(&values[0], &values[1])
    );
}
