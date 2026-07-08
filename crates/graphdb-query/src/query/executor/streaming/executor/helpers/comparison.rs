//! Value comparison for sorting
//!
//! Provides comparison logic for VALUES in streaming executor.

use crate::core::Value;
use std::cmp::Ordering;

/// Compare two values for sorting
///
/// Handles NULL values (NULL comes last), numeric values, strings, and other types.
/// Used in Sort and other comparison operations.
pub fn compare_values(a: &Value, b: &Value) -> Ordering {
    // Handle NULL values - NULL comes last
    match (a, b) {
        (Value::Null(_), Value::Null(_)) => Ordering::Equal,
        (Value::Null(_), _) => Ordering::Greater,
        (_, Value::Null(_)) => Ordering::Less,
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (Value::BigInt(a), Value::BigInt(b)) => a.cmp(b),
        (Value::Float(a), Value::Float(b)) => {
            if a < b {
                Ordering::Less
            } else if a > b {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        (Value::Double(a), Value::Double(b)) => {
            if a < b {
                Ordering::Less
            } else if a > b {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        (Value::String(a), Value::String(b)) => a.cmp(b),
        _ => a.to_string().cmp(&b.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::value::NullType;

    #[test]
    fn test_null_ordering() {
        let null_val = Value::Null(NullType::Null);
        let int_val = Value::Int(1);
        let string_val = Value::String("a".to_string());

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
        let va = Value::String("a".to_string());
        let vb = Value::String("b".to_string());
        let vc = Value::String("c".to_string());

        assert_eq!(compare_values(&va, &vb), Ordering::Less);
        assert_eq!(compare_values(&vb, &va), Ordering::Greater);
        assert_eq!(compare_values(&va, &va), Ordering::Equal);
        assert_eq!(compare_values(&vb, &vc), Ordering::Less);
    }

    #[test]
    fn test_float_ordering() {
        let v1 = Value::Float(1.5);
        let v2 = Value::Float(2.5);
        let v3 = Value::Double(3.14);
        let v4 = Value::Double(2.71);

        assert_eq!(compare_values(&v1, &v2), Ordering::Less);
        assert_eq!(compare_values(&v2, &v1), Ordering::Greater);
        assert_eq!(compare_values(&v1, &v1), Ordering::Equal);
        assert_eq!(compare_values(&v3, &v4), Ordering::Greater);
    }
}

