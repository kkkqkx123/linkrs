//! Value comparison for sorting
//!
//! Provides comparison logic for VALUES in streaming executor.

use linkrs_core::Value;
use std::cmp::Ordering;

/// Compare two values for sorting.
///
/// NULL is explicitly sorted last. Every other shape delegates to `Value`'s
/// total order so sorting, predicate evaluation and grouping agree on cross
/// numeric kinds, dates and NaN placement.
pub fn compare_values(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Null(_), Value::Null(_)) => Ordering::Equal,
        (Value::Null(_), _) => Ordering::Greater,
        (_, Value::Null(_)) => Ordering::Less,
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::SmallInt(a), Value::SmallInt(b)) => a.cmp(b),
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::BigInt(a), Value::BigInt(b)) => a.cmp(b),
        (Value::String(a), Value::String(b)) => a.cmp(b),
        _ => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use linkrs_core::value::NullType;

    #[test]
    fn test_null_ordering() {
        let null_val = Value::Null(NullType::Null);
        let int_val = Value::Int(1);
        let string_val = Value::string("a");

        // NULL should be Greater (sorted last)
        assert_eq!(compare_values(&null_val, &int_val), Ordering::Greater);
        assert_eq!(compare_values(&int_val, &null_val), Ordering::Less);
        assert_eq!(compare_values(&null_val, &null_val), Ordering::Equal);
        assert_eq!(compare_values(&null_val, &string_val), Ordering::Greater);
    }

    #[test]
    fn test_numeric_ordering() {
        let v1 = Value::Int(1);
        let v2 = Value::Int(2);
        let v3 = Value::BigInt(100);
        let v4 = Value::BigInt(99);

        assert_eq!(compare_values(&v1, &v2), Ordering::Less);
        assert_eq!(compare_values(&v2, &v1), Ordering::Greater);
        assert_eq!(compare_values(&v1, &v1), Ordering::Equal);
        assert_eq!(compare_values(&v3, &v4), Ordering::Greater);
        assert_eq!(compare_values(&v4, &v3), Ordering::Less);
    }

    #[test]
    fn test_string_ordering() {
        let va = Value::string("a");
        let vb = Value::string("b");
        let vc = Value::string("c");

        assert_eq!(compare_values(&va, &vb), Ordering::Less);
        assert_eq!(compare_values(&vb, &va), Ordering::Greater);
        assert_eq!(compare_values(&va, &va), Ordering::Equal);
        assert_eq!(compare_values(&vb, &vc), Ordering::Less);
    }

    #[test]
    fn test_float_ordering() {
        let v1 = Value::Float(1.5);
        let v2 = Value::Float(2.5);
        let v3 = Value::Double(std::f64::consts::PI);
        let v4 = Value::Double(2.71);

        assert_eq!(compare_values(&v1, &v2), Ordering::Less);
        assert_eq!(compare_values(&v2, &v1), Ordering::Greater);
        assert_eq!(compare_values(&v1, &v1), Ordering::Equal);
        assert_eq!(compare_values(&v3, &v4), Ordering::Greater);
    }

    #[test]
    fn mixed_numeric_values_use_numeric_order() {
        let small = Value::Int(5);
        let large = Value::BigInt(100);
        assert_eq!(compare_values(&small, &large), Ordering::Less);
        assert_eq!(compare_values(&large, &small), Ordering::Greater);
        assert_eq!(
            compare_values(&Value::Int(5), &Value::Double(5.0)),
            Ordering::Equal
        );
    }

    #[test]
    fn nan_sorts_before_numbers() {
        let nan = Value::Double(f64::NAN);
        assert_eq!(compare_values(&nan, &Value::Double(1.0)), Ordering::Less);
        assert_eq!(compare_values(&Value::Double(1.0), &nan), Ordering::Greater);
    }
}
