//! Memory Estimations for Value Types
//!
//! This module provides memory estimation for the Value enum and related types.

use crate::core::types::memory_estimation::MemoryEstimatable;
use crate::core::value::Value;

impl MemoryEstimatable for Value {
    fn estimate_memory(&self) -> usize {
        let base_size = std::mem::size_of::<Value>();

        match self {
            // Fixed-size types
            Value::Empty
            | Value::Null(_)
            | Value::Bool(_)
            | Value::SmallInt(_)
            | Value::Int(_)
            | Value::BigInt(_)
            | Value::Float(_)
            | Value::Double(_) => base_size,

            // Variable-length string types
            Value::String(s) => base_size + s.capacity(),
            Value::FixedString { data, .. } => base_size + data.capacity(),

            // Binary data
            Value::Blob(b) => base_size + b.capacity(),

            // Complex types with nested memory
            Value::Decimal128(d) => base_size + std::mem::size_of_val(d),

            // DateTime types
            Value::Date(_) | Value::Time(_) | Value::DateTime(_) => base_size,

            // Graph types
            Value::Vertex(v) => base_size + std::mem::size_of_val(v.as_ref()),
            Value::Edge(e) => base_size + std::mem::size_of_val(e.as_ref()),
            Value::Path(p) => base_size + std::mem::size_of_val(p.as_ref()),

            // Collection types (recursive)
            Value::List(list) => {
                base_size + list.iter().map(|v| v.estimate_memory()).sum::<usize>()
            }
            Value::Map(map) => {
                base_size
                    + map
                        .iter()
                        .map(|(k, v)| k.capacity() + v.estimate_memory())
                        .sum::<usize>()
            }
            Value::Set(set) => base_size + set.iter().map(|v| v.estimate_memory()).sum::<usize>(),

            // Geography type
            Value::Geography(g) => base_size + std::mem::size_of_val(g),

            // Vector type
            Value::Vector(v) => base_size + v.estimated_size(),

            // DataSet type
            Value::DataSet(ds) => base_size + ds.as_ref().estimated_size(),

            // JSON types
            Value::Json(j) => base_size + j.estimated_size(),
            Value::JsonB(j) => base_size + j.estimated_size(),

            // UUID type (fixed size, 16 bytes)
            Value::Uuid(_) => base_size,

            // Interval type (fixed size)
            Value::Interval(_) => base_size,
        }
    }
}

/// Helper function to estimate memory for a slice of Values
pub fn estimate_values_memory(values: &[Value]) -> usize {
    values.iter().map(|v| v.estimate_memory()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_size_value() {
        let v = Value::Int(42);
        assert_eq!(v.estimate_memory(), std::mem::size_of::<Value>());
    }

    #[test]
    fn test_string_value() {
        let s = String::with_capacity(100);
        let v = Value::String(s);
        assert_eq!(v.estimate_memory(), std::mem::size_of::<Value>() + 100);
    }

    #[test]
    fn test_list_value() {
        use crate::core::value::list::List;
        let list = vec![
            Value::Int(1),
            Value::Int(2),
            Value::String(String::with_capacity(10)),
        ];
        let v = Value::List(Box::new(List::from(list)));
        let expected = std::mem::size_of::<Value>()
            + std::mem::size_of::<Value>() * 2
            + (std::mem::size_of::<Value>() + 10);
        assert!(v.estimate_memory() >= expected);
    }
}
