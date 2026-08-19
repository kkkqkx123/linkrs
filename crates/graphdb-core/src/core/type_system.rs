//! Type system tool module
//!
//! Provide core functions such as type compatibility checking, type precedence, and type conversion.

use crate::core::value::list::List;
use crate::core::{ArrayTypeInfo, DataType, StructTypeInfo, Value};
use std::sync::Arc;

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
        matches!(type_, DataType::Null | DataType::Empty | DataType::Unknown)
    }

    /// Priority of the obtained type (used for type promotion)
    /// The smaller the priority value, the more "basic" the type is. When a type is upgraded, its priority value increases.
    pub fn get_type_priority(type_: &DataType) -> u8 {
        match type_ {
            DataType::Null | DataType::Empty | DataType::Unknown => 0,
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
            DataType::DateTime => 62,
            DataType::Vertex => 80,
            DataType::Edge => 90,
            DataType::Path => 100,
            DataType::List(_) => 110,
            DataType::Set(_) => 120,
            DataType::Map(_) => 130,
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
            // Parameterized composite types sit above all scalar types.
            DataType::Struct(_) => 220,
            DataType::Array(_) => 221,
        }
    }

    /// Rank of a type within the numeric promotion hierarchy:
    /// `SmallInt < Int < BigInt < Decimal128 < Float < Double`.
    /// Returns `None` for non-numeric types.
    fn numeric_promotion_rank(type_: &DataType) -> Option<u8> {
        match type_ {
            DataType::SmallInt => Some(1),
            DataType::Int => Some(2),
            DataType::BigInt => Some(3),
            DataType::Decimal128 => Some(4),
            DataType::Float => Some(5),
            DataType::Double => Some(6),
            _ => None,
        }
    }

    /// Common supertype of two types following the numeric promotion hierarchy.
    fn common_numeric_type(type1: &DataType, type2: &DataType) -> DataType {
        let rank1 = Self::numeric_promotion_rank(type1);
        let rank2 = Self::numeric_promotion_rank(type2);
        let (Some(r1), Some(r2)) = (rank1, rank2) else {
            return DataType::Empty;
        };
        // Fixed-point / floating-point crossing promotes to Double.
        let is_decimal = type1 == &DataType::Decimal128 || type2 == &DataType::Decimal128;
        let is_float = type1 == &DataType::Float || type2 == &DataType::Float;
        if is_decimal && is_float {
            return DataType::Double;
        }
        if r1 >= r2 {
            type1.clone()
        } else {
            type2.clone()
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

        // Numeric promotion hierarchy.
        if Self::numeric_promotion_rank(type1).is_some()
            && Self::numeric_promotion_rank(type2).is_some()
        {
            return Self::common_numeric_type(type1, type2);
        }

        // Temporal hierarchy: Date promotes to DateTime.
        if (type1 == &DataType::Date && type2 == &DataType::DateTime)
            || (type1 == &DataType::DateTime && type2 == &DataType::Date)
        {
            return DataType::DateTime;
        }

        // Struct: field union with recursive common supertypes (aligns with
        // Ladybug's `combineTypes`).
        if let (DataType::Struct(a), DataType::Struct(b)) = (type1, type2) {
            return Self::combine_struct_types(a, b);
        }

        // Array / List: element common supertype.
        if let (DataType::Array(a), DataType::Array(b)) = (type1, type2) {
            return DataType::Array(Arc::new(ArrayTypeInfo::new(
                Self::get_common_type(a.element.as_ref(), b.element.as_ref()),
                None,
            )));
        }
        if let (DataType::List(a), DataType::List(b)) = (type1, type2) {
            let element = Self::get_common_type(a.as_ref(), b.as_ref());
            return if element == DataType::Empty {
                DataType::List(Box::new(DataType::Empty))
            } else {
                DataType::List(Box::new(element))
            };
        }
        // Array <-> List: an untyped List (Empty element) adopts the Array
        // element type; a typed List unifies with it.
        if let (DataType::Array(a), DataType::List(l)) = (type1, type2) {
            return Self::unify_array_list(a, l);
        }
        if let (DataType::List(l), DataType::Array(a)) = (type1, type2) {
            return Self::unify_array_list(a, l);
        }

        DataType::Empty
    }

    /// Unify an ARRAY and a LIST container to an ARRAY whose element type is
    /// the common supertype of the two elements (an untyped side yields to the
    /// typed side).
    fn unify_array_list(array: &ArrayTypeInfo, list_element: &DataType) -> DataType {
        let element = if array.element.as_ref() == &DataType::Empty {
            list_element.clone()
        } else if list_element == &DataType::Empty {
            array.element.as_ref().clone()
        } else {
            Self::get_common_type(array.element.as_ref(), list_element)
        };
        DataType::Array(Arc::new(ArrayTypeInfo::new(element, None)))
    }

    /// Union two Struct types field-wise: fields present in both take the
    /// recursive common supertype; fields present in only one keep their type.
    /// Fields are ordered by the first type's order, then new fields from the
    /// second type.
    fn combine_struct_types(a: &StructTypeInfo, b: &StructTypeInfo) -> DataType {
        let mut fields: Vec<(String, DataType)> =
            Vec::with_capacity(a.fields.len() + b.fields.len());
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (name, field_type) in &a.fields {
            seen.insert(name.as_str());
            let common = b
                .fields
                .iter()
                .find(|(b_name, _)| b_name == name)
                .map(|(_, b_type)| Self::get_common_type(field_type, b_type))
                .unwrap_or_else(|| field_type.clone());
            fields.push((name.clone(), common));
        }
        for (name, field_type) in &b.fields {
            if !seen.contains(name.as_str()) {
                fields.push((name.clone(), field_type.clone()));
            }
        }
        DataType::Struct(Arc::new(StructTypeInfo::new(fields)))
    }

    /// A Struct is isomorphic to Map when all field names are unique.
    /// (A Map has a single homogeneous value type, which the field common
    /// supertype supplies; no name conflict is allowed.)
    fn struct_isomorphic_to_map(s: &StructTypeInfo) -> bool {
        let mut names: std::collections::HashSet<&str> = std::collections::HashSet::new();
        s.fields.iter().all(|(name, _)| names.insert(name.as_str()))
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
            "+" | "-" | "*" => Self::get_common_type(left_type, right_type),
            // Division always produces a floating-point result so integer
            // division does not silently truncate.
            "/" => match Self::get_common_type(left_type, right_type) {
                DataType::Float => DataType::Float,
                DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::Decimal128
                | DataType::Double => DataType::Double,
                _ => DataType::Empty,
            },
            "==" | "!=" | "<" | "<=" | ">" | ">=" => DataType::Bool,
            _ => DataType::Empty,
        }
    }

    /// Determine whether caching is required (based on complexity heuristics)
    pub fn should_cache_expression(expr_depth: usize, expr_node_count: usize) -> bool {
        expr_depth > 3 || expr_node_count > 10
    }

    /// Check whether the type of the source data can be converted into the target type.
    ///
    /// Hand-written whitelist, grouped by source category:
    /// - integer / floating-point / decimal128 / string cross-conversions
    /// - temporal types (Date/Time/DateTime) to/from String, Date <-> DateTime
    /// - Json <-> JsonB
    /// - Uuid -> String
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

            // Decimal128 converts to/from numeric types and String (lossless).
            (DataType::Decimal128, DataType::Int) => true,
            (DataType::Decimal128, DataType::BigInt) => true,
            (DataType::Decimal128, DataType::Float) => true,
            (DataType::Decimal128, DataType::Double) => true,
            (DataType::Decimal128, DataType::String) => true,
            (DataType::Int, DataType::Decimal128) => true,
            (DataType::BigInt, DataType::Decimal128) => true,
            (DataType::Float, DataType::Decimal128) => true,
            (DataType::Double, DataType::Decimal128) => true,
            (DataType::String, DataType::Decimal128) => true,

            // String can be converted to numeric types and temporal types
            (DataType::String, DataType::SmallInt) => true,
            (DataType::String, DataType::Int) => true,
            (DataType::String, DataType::BigInt) => true,
            (DataType::String, DataType::Float) => true,
            (DataType::String, DataType::Double) => true,
            (DataType::String, DataType::Bool) => true,
            (DataType::String, DataType::Date) => true,
            (DataType::String, DataType::Time) => true,
            (DataType::String, DataType::DateTime) => true,
            (DataType::String, DataType::Uuid) => true,

            // Temporal types convert to String; Date and DateTime inter-convert.
            (DataType::Date, DataType::String) => true,
            (DataType::Date, DataType::DateTime) => true,
            (DataType::Time, DataType::String) => true,
            (DataType::DateTime, DataType::String) => true,
            (DataType::DateTime, DataType::Date) => true,

            // Json <-> JsonB
            (DataType::Json, DataType::JsonB) => true,
            (DataType::JsonB, DataType::Json) => true,

            // Struct <-> Map: isomorphic (same field name set; the Map value type
            // is the common supertype of the field types, which always exists).
            (DataType::Struct(s), DataType::Map(_)) => Self::struct_isomorphic_to_map(s),
            (DataType::Map(_), DataType::Struct(s)) => Self::struct_isomorphic_to_map(s),

            // Array <-> List: the element check happens at cast time; an
            // untyped List accepts every Array and vice versa.
            (DataType::Array(_), DataType::List(_)) => true,
            (DataType::List(_), DataType::Array(_)) => true,
            // Array <-> Array: element types must inter-cast.
            (DataType::Array(a), DataType::Array(b)) => {
                if a.element.as_ref() == &DataType::Empty || b.element.as_ref() == &DataType::Empty
                {
                    true
                } else {
                    Self::can_cast(a.element.as_ref(), b.element.as_ref())
                }
            }

            // Struct/Array -> String: readable serialization.
            (DataType::Struct(_), DataType::String) => true,
            (DataType::Array(_), DataType::String) => true,

            // Uuid -> String
            (DataType::Uuid, DataType::String) => true,

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
                DataType::Decimal128,
                DataType::String,
            ],
            DataType::BigInt => vec![
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::Decimal128,
                DataType::String,
            ],
            DataType::Float => vec![
                DataType::Float,
                DataType::Double,
                DataType::Int,
                DataType::BigInt,
                DataType::Decimal128,
                DataType::String,
            ],
            DataType::Double => vec![
                DataType::Double,
                DataType::Int,
                DataType::BigInt,
                DataType::Decimal128,
                DataType::String,
            ],
            DataType::Decimal128 => vec![
                DataType::Decimal128,
                DataType::Int,
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::String,
            ],
            DataType::String => vec![
                DataType::String,
                DataType::SmallInt,
                DataType::Int,
                DataType::BigInt,
                DataType::Float,
                DataType::Double,
                DataType::Decimal128,
                DataType::Bool,
                DataType::Date,
                DataType::Time,
                DataType::DateTime,
                DataType::Uuid,
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
            DataType::Date => vec![DataType::Date, DataType::String, DataType::DateTime],
            DataType::Time => vec![DataType::Time, DataType::String],
            DataType::DateTime => vec![DataType::DateTime, DataType::String, DataType::Date],
            DataType::Json => vec![DataType::Json, DataType::JsonB],
            DataType::JsonB => vec![DataType::JsonB, DataType::Json],
            DataType::Uuid => vec![DataType::Uuid, DataType::String],
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
            DataType::Unknown => "unknown".to_string(),
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
            DataType::DateTime => "datetime".to_string(),
            DataType::Vertex => "vertex".to_string(),
            DataType::Edge => "edge".to_string(),
            DataType::Path => "path".to_string(),
            DataType::List(element) => {
                if element.as_ref() == &DataType::Empty {
                    "list".to_string()
                } else {
                    format!("list<{}>", element)
                }
            }
            DataType::Map(value) => {
                if value.as_ref() == &DataType::Empty {
                    "map".to_string()
                } else {
                    format!("map<{}>", value)
                }
            }
            DataType::Set(element) => {
                if element.as_ref() == &DataType::Empty {
                    "set".to_string()
                } else {
                    format!("set<{}>", element)
                }
            }
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
            DataType::Struct(_) => "struct".to_string(),
            DataType::Array(_) => "array".to_string(),
        }
    }

    /// Check whether the type can be used for indexing.
    ///
    /// Delegates to `OrderedCodec::supports_ordered_key` so the index-creation
    /// validation (DDL) and the index-write encoding share one source of truth.
    pub fn is_indexable_type(type_def: &DataType) -> bool {
        crate::core::value::ordered_codec::OrderedCodec::supports_ordered_key(type_def)
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
            DataType::String => Some(Value::string("")),
            DataType::List(_) => Some(Value::list(List::from(Vec::new()))),
            DataType::Map(_) => Some(Value::map(std::collections::HashMap::new())),
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

    fn struct_type(fields: Vec<(&str, DataType)>) -> DataType {
        DataType::Struct(Arc::new(StructTypeInfo::new(
            fields
                .into_iter()
                .map(|(n, t)| (n.to_string(), t))
                .collect(),
        )))
    }

    fn array_type(element: DataType) -> DataType {
        DataType::Array(Arc::new(ArrayTypeInfo::new(element, None)))
    }

    #[test]
    fn test_get_common_type_struct_union() {
        // Field intersection takes the common supertype; disjoint fields are
        // kept as-is (union semantics, Ladybug `combineTypes`).
        let a = struct_type(vec![("city", DataType::String), ("age", DataType::Int)]);
        let b = struct_type(vec![
            ("city", DataType::String),
            ("age", DataType::BigInt),
            ("street", DataType::String),
        ]);
        let common = TypeUtils::get_common_type(&a, &b);
        assert_eq!(
            common,
            struct_type(vec![
                ("city", DataType::String),
                ("age", DataType::BigInt),
                ("street", DataType::String),
            ])
        );

        // Nested Struct fields recurse.
        let nested_a = struct_type(vec![("geo", struct_type(vec![("lat", DataType::Float)]))]);
        let nested_b = struct_type(vec![("geo", struct_type(vec![("lat", DataType::Double)]))]);
        let common_nested = TypeUtils::get_common_type(&nested_a, &nested_b);
        assert_eq!(
            common_nested,
            struct_type(vec![("geo", struct_type(vec![("lat", DataType::Double)]),)])
        );

        // Heterogeneous Struct types with no common fields still unify.
        let disjoint_a = struct_type(vec![("a", DataType::Int)]);
        let disjoint_b = struct_type(vec![("b", DataType::Int)]);
        let common_disjoint = TypeUtils::get_common_type(&disjoint_a, &disjoint_b);
        assert_eq!(
            common_disjoint,
            struct_type(vec![("a", DataType::Int), ("b", DataType::Int)])
        );
    }

    #[test]
    fn test_get_common_type_array_and_list() {
        let untyped_list = DataType::List(Box::new(DataType::Empty));
        assert_eq!(
            TypeUtils::get_common_type(&array_type(DataType::Int), &array_type(DataType::Float)),
            array_type(DataType::Float)
        );
        // Array + untyped List unify to an Array of the element type.
        assert_eq!(
            TypeUtils::get_common_type(&array_type(DataType::Int), &untyped_list),
            array_type(DataType::Int)
        );
        assert_eq!(
            TypeUtils::get_common_type(&untyped_list, &array_type(DataType::Int)),
            array_type(DataType::Int)
        );
        // Typed List + List unify to a List of the common element type.
        assert_eq!(
            TypeUtils::get_common_type(
                &DataType::List(Box::new(DataType::Int)),
                &DataType::List(Box::new(DataType::Float)),
            ),
            DataType::List(Box::new(DataType::Float))
        );
        // Typed List + Array unify to an Array of the common element type.
        assert_eq!(
            TypeUtils::get_common_type(
                &DataType::List(Box::new(DataType::Int)),
                &array_type(DataType::Float),
            ),
            array_type(DataType::Float)
        );
        // Struct and Array do not unify.
        assert_eq!(
            TypeUtils::get_common_type(&struct_type(vec![]), &array_type(DataType::Int)),
            DataType::Empty
        );
    }

    #[test]
    fn test_can_cast_struct_array_rules() {
        use std::sync::Arc;

        let person = struct_type(vec![("name", DataType::String), ("age", DataType::Int)]);
        let doubles = array_type(DataType::Double);
        let map_ty = DataType::Map(Box::new(DataType::Empty));
        let list_ty = DataType::List(Box::new(DataType::Empty));

        // Struct <-> Map (isomorphic).
        assert!(TypeUtils::can_cast(&person, &map_ty));
        assert!(TypeUtils::can_cast(&map_ty, &person));
        // Duplicate field names break isomorphism.
        let dup = DataType::Struct(Arc::new(StructTypeInfo::new(vec![
            ("x".to_string(), DataType::Int),
            ("x".to_string(), DataType::Int),
        ])));
        assert!(!TypeUtils::can_cast(&dup, &map_ty));

        // Array <-> List.
        assert!(TypeUtils::can_cast(&doubles, &list_ty));
        assert!(TypeUtils::can_cast(&list_ty, &doubles));
        // Array -> Array with inter-castable elements.
        assert!(TypeUtils::can_cast(
            &array_type(DataType::Int),
            &array_type(DataType::Float)
        ));
        assert!(!TypeUtils::can_cast(
            &array_type(DataType::Int),
            &array_type(DataType::Date)
        ));

        // Struct/Array -> String.
        assert!(TypeUtils::can_cast(&person, &DataType::String));
        assert!(TypeUtils::can_cast(&doubles, &DataType::String));

        // No reverse: String -> Struct.
        assert!(!TypeUtils::can_cast(&DataType::String, &person));
        // List -> Array -> List is allowed, but Struct/List stays disjoint.
        assert!(!TypeUtils::can_cast(&person, &list_ty));
    }

    #[test]
    fn test_get_common_type_numeric_promotion() {
        // SmallInt < Int < BigInt < Decimal128 < Float < Double
        assert_eq!(
            TypeUtils::get_common_type(&DataType::SmallInt, &DataType::Int),
            DataType::Int
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Int, &DataType::BigInt),
            DataType::BigInt
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::BigInt, &DataType::Decimal128),
            DataType::Decimal128
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Int, &DataType::Decimal128),
            DataType::Decimal128
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Decimal128, &DataType::Float),
            DataType::Double
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Decimal128, &DataType::Double),
            DataType::Double
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Int, &DataType::Float),
            DataType::Float
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Float, &DataType::Double),
            DataType::Double
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Int, &DataType::Int),
            DataType::Int
        );
        // String and Blob do not promote.
        assert_eq!(
            TypeUtils::get_common_type(&DataType::String, &DataType::Blob),
            DataType::Empty
        );
        // Temporal hierarchy.
        assert_eq!(
            TypeUtils::get_common_type(&DataType::Date, &DataType::DateTime),
            DataType::DateTime
        );
        assert_eq!(
            TypeUtils::get_common_type(&DataType::DateTime, &DataType::Date),
            DataType::DateTime
        );
    }

    #[test]
    fn test_binary_operation_result_type_numeric_promotion() {
        assert_eq!(
            TypeUtils::binary_operation_result_type("+", &DataType::Int, &DataType::Int),
            DataType::Int
        );
        assert_eq!(
            TypeUtils::binary_operation_result_type("+", &DataType::Int, &DataType::Float),
            DataType::Float
        );
        assert_eq!(
            TypeUtils::binary_operation_result_type("*", &DataType::BigInt, &DataType::Decimal128),
            DataType::Decimal128
        );
        assert_eq!(
            TypeUtils::binary_operation_result_type("+", &DataType::Decimal128, &DataType::Float),
            DataType::Double
        );
        assert_eq!(
            TypeUtils::binary_operation_result_type("-", &DataType::SmallInt, &DataType::BigInt),
            DataType::BigInt
        );
        // Division always produces a floating-point result.
        assert_eq!(
            TypeUtils::binary_operation_result_type("/", &DataType::Int, &DataType::Int),
            DataType::Double
        );
        assert_eq!(
            TypeUtils::binary_operation_result_type("/", &DataType::Decimal128, &DataType::Int),
            DataType::Double
        );
        assert_eq!(
            TypeUtils::binary_operation_result_type("/", &DataType::Float, &DataType::Float),
            DataType::Float
        );
        // Non-numeric arithmetic has no result type.
        assert_eq!(
            TypeUtils::binary_operation_result_type("+", &DataType::String, &DataType::Int),
            DataType::Empty
        );
        assert_eq!(
            TypeUtils::binary_operation_result_type("==", &DataType::Int, &DataType::Int),
            DataType::Bool
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
            TypeUtils::literal_type(&Value::string("test")),
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
    fn test_can_cast_extended_matrix() {
        // Decimal128 <-> numeric / string
        assert!(TypeUtils::can_cast(&DataType::Decimal128, &DataType::Int));
        assert!(TypeUtils::can_cast(
            &DataType::Decimal128,
            &DataType::BigInt
        ));
        assert!(TypeUtils::can_cast(&DataType::Decimal128, &DataType::Float));
        assert!(TypeUtils::can_cast(
            &DataType::Decimal128,
            &DataType::Double
        ));
        assert!(TypeUtils::can_cast(
            &DataType::Decimal128,
            &DataType::String
        ));
        assert!(TypeUtils::can_cast(&DataType::Int, &DataType::Decimal128));
        assert!(TypeUtils::can_cast(
            &DataType::BigInt,
            &DataType::Decimal128
        ));
        assert!(TypeUtils::can_cast(&DataType::Float, &DataType::Decimal128));
        assert!(TypeUtils::can_cast(
            &DataType::Double,
            &DataType::Decimal128
        ));
        assert!(TypeUtils::can_cast(
            &DataType::String,
            &DataType::Decimal128
        ));
        assert!(!TypeUtils::can_cast(&DataType::Decimal128, &DataType::Bool));

        // Temporal types
        assert!(TypeUtils::can_cast(&DataType::DateTime, &DataType::String));
        assert!(TypeUtils::can_cast(&DataType::Date, &DataType::String));
        assert!(TypeUtils::can_cast(&DataType::Time, &DataType::String));
        assert!(TypeUtils::can_cast(&DataType::String, &DataType::Time));
        assert!(TypeUtils::can_cast(&DataType::String, &DataType::Uuid));
        assert!(TypeUtils::can_cast(&DataType::Date, &DataType::DateTime));
        assert!(TypeUtils::can_cast(&DataType::DateTime, &DataType::Date));
        assert!(!TypeUtils::can_cast(&DataType::Time, &DataType::Date));

        // Json <-> JsonB
        assert!(TypeUtils::can_cast(&DataType::Json, &DataType::JsonB));
        assert!(TypeUtils::can_cast(&DataType::JsonB, &DataType::Json));

        // Uuid -> String
        assert!(TypeUtils::can_cast(&DataType::Uuid, &DataType::String));
        assert!(!TypeUtils::can_cast(&DataType::String, &DataType::Json));
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
        assert!(!TypeUtils::is_indexable_type(&DataType::List(Box::new(
            DataType::Int
        ))));
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
            Some(Value::string(""))
        );
        assert!(TypeUtils::get_default_value(&DataType::Date).is_none());
    }
}
