//! Type system tool module
//!
//! Provide core functions such as type compatibility checking, type precedence, and type conversion.

use crate::core::value::list::List;
use crate::core::DataType;
use crate::core::Value;

/// Type system tools
pub struct TypeUtils;

impl TypeUtils {
    /// Check whether the two types are compatible.
    pub fn are_types_compatible(type1: &DataType, type2: &DataType) -> bool {
        if type1 == type2 {
            return true;
        }

        if Self::is_superior_type(type1) || Self::is_superior_type(type2) {
            return true;
        }

        if (type1 == &DataType::Int && type2 == &DataType::Float)
            || (type1 == &DataType::Float && type2 == &DataType::Int)
        {
            return true;
        }

        false
    }

    /// Check whether the type is a "superior type" (which can be compatible with any other type).
    pub fn is_superior_type(type_: &DataType) -> bool {
        matches!(type_, DataType::Null | DataType::Empty)
    }

    /// Priority of the obtained type (used for type promotion)
    /// The smaller the priority value, the more "basic" the type is. When a type is upgraded, its priority value increases.
    pub fn get_type_priority(type_: &DataType) -> u8 {
        match type_ {
            DataType::Null | DataType::Empty => 0,
            DataType::Bool => 10,
            DataType::SmallInt => 20,
            DataType::Int => 21,
            DataType::BigInt => 22,
            DataType::Float => 30,
            DataType::Double => 31,
            DataType::Decimal128 => 32,
            DataType::String => 40,
            DataType::FixedString(_) => 41,
            DataType::Date => 50,
            DataType::Time => 60,
            DataType::Timestamp => 61,
            DataType::DateTime => 62,
            DataType::VID => 70,
            DataType::Vertex => 80,
            DataType::Edge => 90,
            DataType::Path => 100,
            DataType::List => 110,
            DataType::Set => 120,
            DataType::Map => 130,
            DataType::Blob => 140,
            DataType::Geography => 150,
            DataType::DataSet => 160,
            DataType::Vector => 180,
            DataType::VectorDense(_) => 181,
            DataType::VectorSparse(_) => 182,
            DataType::Json => 190,
            DataType::JsonB => 191,
            DataType::Uuid => 200,
            DataType::Interval => 210,
        }
    }

    /// Obtaining two types of common supertypes
    pub fn get_common_type(type1: &DataType, type2: &DataType) -> DataType {
        if type1 == type2 {
            return type1.clone();
        }

        if Self::is_superior_type(type1) {
            return type2.clone();
        }
        if Self::is_superior_type(type2) {
            return type1.clone();
        }

        if (type1 == &DataType::Int && type2 == &DataType::Float)
            || (type1 == &DataType::Float && type2 == &DataType::Int)
        {
            return DataType::Float;
        }

        DataType::Empty
    }

    /// Unified type compatibility checks (without the need for caching)
    pub fn check_compatibility(type1: &DataType, type2: &DataType) -> bool {
        Self::are_types_compatible(type1, type2)
    }

    /// Batch type checking (for optimizing memory allocation)
    pub fn check_compatibility_batch(pairs: &[(DataType, DataType)]) -> Vec<bool> {
        let mut results = Vec::with_capacity(pairs.len());

        for (t1, t2) in pairs {
            results.push(Self::check_compatibility(t1, t2));
        }
        results
    }

    /// Obtaining the literal value type
    pub fn literal_type(value: &crate::core::value::Value) -> DataType {
        value.get_type()
    }

    /// Type of the result of a binary operation
    pub fn binary_operation_result_type(
        op: &str,
        left_type: &DataType,
        right_type: &DataType,
    ) -> DataType {
        match op {
            "+" | "-" | "*" | "/" => {
                if left_type == &DataType::Float || right_type == &DataType::Float {
                    DataType::Float
                } else {
                    DataType::Int
                }
            }
            "==" | "!=" | "<" | "<=" | ">" | ">=" => DataType::Bool,
            _ => DataType::Empty,
        }
    }

    /// Determine whether caching is required (based on complexity heuristics)
    pub fn should_cache_expression(expr_depth: usize, expr_node_count: usize) -> bool {
        expr_depth > 3 || expr_node_count > 10
    }

    /// Check whether the type of the source data can be converted into the target type.
    pub fn can_cast(from: &DataType, to: &DataType) -> bool {
        if from == to {
            return true;
        }

        match (from, to) {
            // Integer types can be converted to Int, Float, or String
            (DataType::SmallInt, DataType::Int) => true,
            (DataType::SmallInt, DataType::BigInt) => true,
            (DataType::SmallInt, DataType::Float) => true,
            (DataType::SmallInt, DataType::Double) => true,
            (DataType::SmallInt, DataType::String) => true,
            (DataType::Int, DataType::BigInt) => true,
            (DataType::Int, DataType::Float) => true,
            (DataType::Int, DataType::Double) => true,
            (DataType::Int, DataType::String) => true,
            (DataType::BigInt, DataType::Float) => true,
            (DataType::BigInt, DataType::Double) => true,
            (DataType::BigInt, DataType::String) => true,

            // Float types can be converted to Int or String
            (DataType::Float, DataType::Double) => true,
            (DataType::Float, DataType::Int) => true,
            (DataType::Float, DataType::BigInt) => true,
            (DataType::Float, DataType::String) => true,
            (DataType::Double, DataType::Int) => true,
            (DataType::Double, DataType::BigInt) => true,
            (DataType::Double, DataType::String) => true,

            // String can be converted to numeric types
            (DataType::String, DataType::SmallInt) => true,
            (DataType::String, DataType::Int) => true,
            (DataType::String, DataType::BigInt) => true,
            (DataType::String, DataType::Float) => true,
            (DataType::String, DataType::Double) => true,
            (DataType::String, DataType::Bool) => true,
            (DataType::String, DataType::Date) => true,
            (DataType::String, DataType::DateTime) => true,

            // FixedString can be converted to various types
            (DataType::FixedString(_), DataType::String) => true,
            (DataType::FixedString(_), DataType::SmallInt) => true,
            (DataType::FixedString(_), DataType::Int) => true,
            (DataType::FixedString(_), DataType::BigInt) => true,
            (DataType::FixedString(_), DataType::Float) => true,
            (DataType::FixedString(_), DataType::Double) => true,
            (DataType::FixedString(_), DataType::Bool) => true,
            (DataType::FixedString(_), DataType::Date) => true,
            (DataType::FixedString(_), DataType::DateTime) => true,

            // Bool can be converted to numeric types
            (DataType::Bool, DataType::SmallInt) => true,
            (DataType::Bool, DataType::Int) => true,
            (DataType::Bool, DataType::BigInt) => true,
            (DataType::Bool, DataType::Float) => true,
            (DataType::Bool, DataType::Double) => true,
            (DataType::Bool, DataType::String) => true,

            // Null can be converted to any type
            (DataType::Null, _) => true,

            // Empty can be converted to basic types
            (DataType::Empty, DataType::Empty) => true,
            (DataType::Empty, DataType::Bool) => true,
            (DataType::Empty, DataType::SmallInt) => true,
            (DataType::Empty, DataType::Int) => true,
            (DataType::Empty, DataType::BigInt) => true,
            (DataType::Empty, DataType::Float) => true,
            (DataType::Empty, DataType::Double) => true,
            (DataType::Empty, DataType::String) => true,

            _ => false,
        }
    }

    /// The list of source types that can be converted into all possible target types
    pub fn get_cast_targets(from: &DataType) -> Vec<DataType> {
        match from {
            DataType::SmallInt => vec![
                DataType::SmallInt,
                DataType::Int,
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::String,
            ],
            DataType::Int => vec![
                DataType::Int,
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::String,
            ],
            DataType::BigInt => vec![
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::String,
            ],
            DataType::Float => vec![
                DataType::Float,
                DataType::Double,
                DataType::Int,
                DataType::BigInt,
                DataType::String,
            ],
            DataType::Double => vec![
                DataType::Double,
                DataType::Int,
                DataType::BigInt,
                DataType::String,
            ],
            DataType::String => vec![
                DataType::String,
                DataType::SmallInt,
                DataType::Int,
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::Bool,
                DataType::Date,
                DataType::DateTime,
            ],
            DataType::FixedString(_) => vec![
                DataType::String,
                DataType::SmallInt,
                DataType::Int,
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::Bool,
                DataType::Date,
                DataType::DateTime,
            ],
            DataType::Bool => vec![
                DataType::Bool,
                DataType::SmallInt,
                DataType::Int,
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::String,
            ],
            DataType::Null => vec![
                DataType::Null,
                DataType::SmallInt,
                DataType::Int,
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::String,
                DataType::Bool,
            ],
            DataType::Empty => vec![
                DataType::Empty,
                DataType::Bool,
                DataType::SmallInt,
                DataType::Int,
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::String,
            ],
            // Other types can only be converted into themselves.
            _ => vec![from.clone()],
        }
    }

    /// Verify whether the type conversion is valid (based on NebulaGraph design)
    pub fn validate_type_cast(from: &DataType, to: &DataType) -> bool {
        Self::can_cast(from, to)
    }

    /// The string representation of the obtained type.
    pub fn type_to_string(type_def: &DataType) -> String {
        match type_def {
            DataType::Empty => "empty".to_string(),
            DataType::Null => "null".to_string(),
            DataType::Bool => "bool".to_string(),
            DataType::SmallInt => "smallint".to_string(),
            DataType::Int => "int".to_string(),
            DataType::BigInt => "bigint".to_string(),
            DataType::Float => "float".to_string(),
            DataType::Double => "double".to_string(),
            DataType::Decimal128 => "decimal128".to_string(),
            DataType::String => "string".to_string(),
            DataType::FixedString(len) => format!("fixed_string({})", len),
            DataType::Date => "date".to_string(),
            DataType::Time => "time".to_string(),
            DataType::Timestamp => "timestamp".to_string(),
            DataType::DateTime => "datetime".to_string(),
            DataType::VID => "vid".to_string(),
            DataType::Vertex => "vertex".to_string(),
            DataType::Edge => "edge".to_string(),
            DataType::Path => "path".to_string(),
            DataType::List => "list".to_string(),
            DataType::Map => "map".to_string(),
            DataType::Set => "set".to_string(),
            DataType::Blob => "blob".to_string(),
            DataType::Geography => "geography".to_string(),
            DataType::DataSet => "dataset".to_string(),
            DataType::Vector => "vector".to_string(),
            DataType::VectorDense(dim) => format!("vector_dense({})", dim),
            DataType::VectorSparse(dim) => format!("vector_sparse({})", dim),
            DataType::Json => "json".to_string(),
            DataType::JsonB => "jsonb".to_string(),
            DataType::Uuid => "uuid".to_string(),
            DataType::Interval => "interval".to_string(),
        }
    }

    /// Check whether the type can be used for indexing.
    pub fn is_indexable_type(type_def: &DataType) -> bool {
        matches!(
            type_def,
            DataType::Bool
                | DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::Float
                | DataType::Double
                | DataType::String
                | DataType::FixedString(_)
                | DataType::DateTime
                | DataType::Date
                | DataType::Time
                | DataType::Timestamp
                | DataType::VID
                | DataType::Blob
                | DataType::Geography
        )
    }

    /// Get the default value of the type.
    pub fn get_default_value(type_def: &DataType) -> Option<Value> {
        match type_def {
            DataType::Bool => Some(Value::Bool(false)),
            DataType::SmallInt => Some(Value::SmallInt(0)),
            DataType::Int => Some(Value::Int(0)),
            DataType::BigInt => Some(Value::BigInt(0)),
            DataType::Float => Some(Value::Float(0.0)),
            DataType::Double => Some(Value::Double(0.0)),
            DataType::String => Some(Value::String(String::new())),
            DataType::List => Some(Value::list(List::from(Vec::new()))),
            DataType::Map => Some(Value::map(std::collections::HashMap::new())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_are_types_compatible() {
        assert!(TypeUtils::are_types_compatible(
            &DataType::Int,
            &DataType::Int
        ));

        assert!(TypeUtils::are_types_compatible(
            &DataType::Null,
            &DataType::Int
        ));
        assert!(TypeUtils::are_types_compatible(
            &DataType::Empty,
            &DataType::String
        ));

        assert!(TypeUtils::are_types_compatible(
            &DataType::Int,
            &DataType::Float
        ));
        assert!(TypeUtils::are_types_compatible(
            &DataType::Float,
            &DataType::Int
        ));

        assert!(!TypeUtils::are_types_compatible(
            &DataType::Int,
            &DataType::String
        ));
    }

    #[test]
    fn test_is_superior_type() {
        assert!(TypeUtils::is_superior_type(&DataType::Null));
        assert!(TypeUtils::is_superior_type(&DataType::Empty));
        assert!(!TypeUtils::is_superior_type(&DataType::Int));
        assert!(!TypeUtils::is_superior_type(&DataType::String));
    }

    #[test]
    fn test_get_type_priority() {
        assert_eq!(TypeUtils::get_type_priority(&DataType::Null), 0);
        assert_eq!(TypeUtils::get_type_priority(&DataType::Int), 21);
        assert_eq!(TypeUtils::get_type_priority(&DataType::Float), 30);
        assert_eq!(TypeUtils::get_type_priority(&DataType::String), 40);
    }

    #[test]
    fn test_get_common_type() {
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Int, &DataType::Float),
            DataType::Float
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Null, &DataType::String),
            DataType::String
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Int, &DataType::String),
            DataType::Empty
        );
    }

    #[test]
    fn test_check_compatibility() {
        assert!(TypeUtils::check_compatibility(
            &DataType::Int,
            &DataType::Int
        ));
        assert!(TypeUtils::check_compatibility(
            &DataType::Int,
            &DataType::Float
        ));
        assert!(!TypeUtils::check_compatibility(
            &DataType::Int,
            &DataType::String
        ));
    }

    #[test]
    fn test_check_compatibility_batch() {
        let pairs = vec![
            (DataType::Int, DataType::Int),
            (DataType::Int, DataType::Float),
            (DataType::Int, DataType::String),
            (DataType::Null, DataType::Int),
        ];

        let results = TypeUtils::check_compatibility_batch(&pairs);
        assert_eq!(results.len(), 4);
        assert!(results[0]);
        assert!(results[1]);
        assert!(!results[2]);
        assert!(results[3]);
    }

    #[test]
    fn test_literal_type() {
        use crate::core::value::Value;
        use std::f64::consts::PI;

        assert_eq!(TypeUtils::literal_type(&Value::Int(42)), DataType::Int);
        assert_eq!(
            TypeUtils::literal_type(&Value::Double(PI)),
            DataType::Double
        );
        assert_eq!(
            TypeUtils::literal_type(&Value::String("test".to_string())),
            DataType::String
        );
    }

    #[test]
    fn test_binary_operation_result_type() {
        assert_eq!(
            TypeUtils::binary_operation_result_type("+", &DataType::Int, &DataType::Int),
            DataType::Int
        );
        assert_eq!(
            TypeUtils::binary_operation_result_type("+", &DataType::Int, &DataType::Float),
            DataType::Float
        );
        assert_eq!(
            TypeUtils::binary_operation_result_type("==", &DataType::Int, &DataType::Int),
            DataType::Bool
        );
    }

    #[test]
    fn test_should_cache_expression() {
        assert!(!TypeUtils::should_cache_expression(2, 5));
        assert!(TypeUtils::should_cache_expression(4, 5));
        assert!(TypeUtils::should_cache_expression(2, 15));
    }

    #[test]
    fn test_can_cast() {
        // The same type
        assert!(TypeUtils::can_cast(&DataType::Int, &DataType::Int));
        assert!(TypeUtils::can_cast(&DataType::String, &DataType::String));

        // Int conversion
        assert!(TypeUtils::can_cast(&DataType::Int, &DataType::Float));
        assert!(TypeUtils::can_cast(&DataType::Int, &DataType::String));
        assert!(!TypeUtils::can_cast(&DataType::Int, &DataType::Bool));

        // Float conversion
        assert!(TypeUtils::can_cast(&DataType::Float, &DataType::Int));
        assert!(TypeUtils::can_cast(&DataType::Float, &DataType::String));
        assert!(!TypeUtils::can_cast(&DataType::Float, &DataType::Bool));

        // String conversion
        assert!(TypeUtils::can_cast(&DataType::String, &DataType::Int));
        assert!(TypeUtils::can_cast(&DataType::String, &DataType::Float));
        assert!(TypeUtils::can_cast(&DataType::String, &DataType::Bool));
        assert!(TypeUtils::can_cast(&DataType::String, &DataType::Date));

        // Bool conversion
        assert!(TypeUtils::can_cast(&DataType::Bool, &DataType::Int));
        assert!(TypeUtils::can_cast(&DataType::Bool, &DataType::String));
        assert!(TypeUtils::can_cast(&DataType::Bool, &DataType::Float));

        // Null conversion
        assert!(TypeUtils::can_cast(&DataType::Null, &DataType::Int));
        assert!(TypeUtils::can_cast(&DataType::Null, &DataType::String));

        // Empty conversion
        assert!(TypeUtils::can_cast(&DataType::Empty, &DataType::Int));
        assert!(TypeUtils::can_cast(&DataType::Empty, &DataType::String));
    }

    #[test]
    fn test_get_cast_targets() {
        let int_targets = TypeUtils::get_cast_targets(&DataType::Int);
        assert!(int_targets.contains(&DataType::Int));
        assert!(int_targets.contains(&DataType::Float));
        assert!(int_targets.contains(&DataType::String));

        let string_targets = TypeUtils::get_cast_targets(&DataType::String);
        assert!(string_targets.contains(&DataType::String));
        assert!(string_targets.contains(&DataType::Int));
        assert!(string_targets.contains(&DataType::Float));
    }

    #[test]
    fn test_validate_type_cast() {
        assert!(TypeUtils::validate_type_cast(
            &DataType::Int,
            &DataType::Float
        ));
        assert!(!TypeUtils::validate_type_cast(
            &DataType::Int,
            &DataType::Bool
        ));
    }

    #[test]
    fn test_type_to_string() {
        assert_eq!(TypeUtils::type_to_string(&DataType::Int), "int");
        assert_eq!(TypeUtils::type_to_string(&DataType::Float), "float");
        assert_eq!(TypeUtils::type_to_string(&DataType::String), "string");
        assert_eq!(
            TypeUtils::type_to_string(&DataType::FixedString(100)),
            "fixed_string(100)"
        );
    }

    #[test]
    fn test_is_indexable_type() {
        assert!(TypeUtils::is_indexable_type(&DataType::Int));
        assert!(TypeUtils::is_indexable_type(&DataType::String));
        assert!(!TypeUtils::is_indexable_type(&DataType::Null));
        assert!(!TypeUtils::is_indexable_type(&DataType::List));
    }

    #[test]
    fn test_get_default_value() {
        assert_eq!(
            TypeUtils::get_default_value(&DataType::Int),
            Some(Value::Int(0))
        );
        assert_eq!(
            TypeUtils::get_default_value(&DataType::Bool),
            Some(Value::Bool(false))
        );
        assert_eq!(
            TypeUtils::get_default_value(&DataType::String),
            Some(Value::String(String::new()))
        );
        assert!(TypeUtils::get_default_value(&DataType::Date).is_none());
    }
}
