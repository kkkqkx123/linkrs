//! Type-level metadata for parameterized data types (aligned with Ladybug's
//! `ExtraTypeInfo` design).
//!
//! `DataType` itself is a flat code; nested type information (STRUCT fields,
//! ARRAY element types, DECIMAL precision/scale) lives here. `Struct`/`Array`
//! variants of `DataType` hold an `Arc` to share the metadata across schema
//! references without deep copies; recursion is broken by the `Arc`/`Box`.

use super::DataType;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Type-level metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TypeInfo {
    /// STRUCT: named-field composite type (fields preserve declaration order).
    Struct(StructTypeInfo),
    /// ARRAY: fixed-length (`len = Some(n)`) or variable-length
    /// (`len = None`, equivalent to a LIST constraint).
    Array(ArrayTypeInfo),
    /// DECIMAL128 precision/scale.
    Decimal { precision: u8, scale: u8 },
}

/// STRUCT field list (order-preserving).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StructTypeInfo {
    pub fields: Vec<(String, DataType)>,
}

/// ARRAY element type and optional fixed length.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArrayTypeInfo {
    pub element: Box<DataType>,
    /// `Some(n)` = fixed-length ARRAY(n).
    pub len: Option<usize>,
}

impl StructTypeInfo {
    pub fn new(fields: Vec<(String, DataType)>) -> Self {
        Self { fields }
    }
}

impl ArrayTypeInfo {
    pub fn new(element: DataType, len: Option<usize>) -> Self {
        Self {
            element: Box::new(element),
            len,
        }
    }
}

impl TypeInfo {
    pub fn struct_(fields: Vec<(String, DataType)>) -> Self {
        TypeInfo::Struct(StructTypeInfo::new(fields))
    }

    pub fn array(element: DataType, len: Option<usize>) -> Self {
        TypeInfo::Array(ArrayTypeInfo::new(element, len))
    }
}

/// Extract the `TypeInfo` from a `DataType` variant that carries one
/// (`Struct`/`Array`). Returns `None` for parameter-free types.
pub fn type_info_of(data_type: &DataType) -> Option<TypeInfo> {
    match data_type {
        DataType::Struct(info) => Some(TypeInfo::Struct(info.as_ref().clone())),
        DataType::Array(info) => Some(TypeInfo::Array(info.as_ref().clone())),
        _ => None,
    }
}

/// Rebuild a `DataType` from its `TypeInfo` if the variant is parameterized.
pub fn data_type_from_info(code: u8, info: &TypeInfo) -> Option<DataType> {
    match (code, info) {
        (64, TypeInfo::Struct(s)) => Some(DataType::Struct(Arc::new(s.clone()))),
        (65, TypeInfo::Array(a)) => Some(DataType::Array(Arc::new(a.clone()))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_struct_type_info_roundtrip() {
        let info = TypeInfo::struct_(vec![
            ("city".to_string(), DataType::String),
            (
                "geo".to_string(),
                DataType::Struct(Arc::new(StructTypeInfo::new(vec![(
                    "lat".to_string(),
                    DataType::Double,
                )]))),
            ),
        ]);
        let encoded = postcard::to_allocvec(&info).expect("encode type info");
        let decoded: TypeInfo = postcard::from_bytes(&encoded).expect("decode type info");
        assert_eq!(decoded, info);
    }

    #[test]
    fn test_array_type_info_roundtrip() {
        let info = TypeInfo::array(DataType::Double, Some(3));
        let encoded = postcard::to_allocvec(&info).expect("encode type info");
        let decoded: TypeInfo = postcard::from_bytes(&encoded).expect("decode type info");
        assert_eq!(decoded, info);
    }

    #[test]
    fn test_data_type_from_info_mismatched_code() {
        let info = TypeInfo::array(DataType::Int, None);
        assert!(data_type_from_info(64, &info).is_none());
    }
}
