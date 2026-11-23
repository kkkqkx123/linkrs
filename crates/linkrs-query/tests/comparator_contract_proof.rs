//! Proof that streaming sorting uses the same value order as predicates and
//! grouping. NULL is deliberately moved last for sorting; every other shape
//! must agree with `Value`'s total order.

use linkrs_core::value::NullType;
use linkrs_core::Value;
use linkrs_query::executor::streaming::helpers::compare_values;
use std::cmp::Ordering;

fn values() -> Vec<Value> {
    vec![
        Value::SmallInt(-1),
        Value::Int(5),
        Value::BigInt(5),
        Value::BigInt(100),
        Value::Float(5.0),
        Value::Double(5.0),
        Value::Float(f32::NAN),
        Value::Double(f64::NAN),
        Value::string("a"),
        Value::Null(NullType::Null),
    ]
}

#[test]
fn non_null_sort_order_matches_value_order() {
    let values: Vec<Value> = values()
        .into_iter()
        .filter(|value| !matches!(value, Value::Null(_)))
        .collect();
    for a in &values {
        for b in &values {
            assert_eq!(
                compare_values(a, b),
                a.cmp(b),
                "sort/predicate mismatch for {a:?} vs {b:?}"
            );
            assert_eq!(
                compare_values(a, b),
                compare_values(b, a).reverse(),
                "sort antisymmetry mismatch for {a:?} vs {b:?}"
            );
        }
    }
}

#[test]
fn null_is_the_only_sort_specific_override() {
    let null = Value::Null(NullType::Null);
    let number = Value::Int(5);
    let string = Value::string("a");

    assert_eq!(compare_values(&null, &null), Ordering::Equal);
    assert_eq!(compare_values(&null, &number), Ordering::Greater);
    assert_eq!(compare_values(&number, &null), Ordering::Less);
    assert_eq!(compare_values(&null, &string), Ordering::Greater);
    assert_eq!(compare_values(&string, &null), Ordering::Less);
}
