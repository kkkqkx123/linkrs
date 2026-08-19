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
    /// LIST: homogeneous element type.
    List(Box<DataType>),
    /// MAP: shared value type (string keys).
    Map(Box<DataType>),
    /// SET: homogeneous element type.
    Set(Box<DataType>),
    /// STRUCT: named-field composite type (fields preserve declaration order).
    Struct(StructTypeInfo),
    /// ARRAY: fixed-length (`len = Some(n)`) or variable-length
    /// (`len = None`, equivalent to a LIST constraint).
    Array(ArrayTypeInfo),
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
    pub fn list(element: DataType) -> Self {
        TypeInfo::List(Box::new(element))
    }

    pub fn map(value: DataType) -> Self {
        TypeInfo::Map(Box::new(value))
    }

    pub fn set(element: DataType) -> Self {
        TypeInfo::Set(Box::new(element))
    }

    pub fn struct_(fields: Vec<(String, DataType)>) -> Self {
        TypeInfo::Struct(StructTypeInfo::new(fields))
    }

    pub fn array(element: DataType, len: Option<usize>) -> Self {
        TypeInfo::Array(ArrayTypeInfo::new(element, len))
    }
}

/// Extract the `TypeInfo` from a `DataType` variant that carries one
/// (`List`/`Map`/`Set`/`Struct`/`Array`). Returns `None` for parameter-free
/// types.
pub fn type_info_of(data_type: &DataType) -> Option<TypeInfo> {
    match data_type {
        DataType::List(element) => Some(TypeInfo::List(element.clone())),
        DataType::Map(value) => Some(TypeInfo::Map(value.clone())),
        DataType::Set(element) => Some(TypeInfo::Set(element.clone())),
        DataType::Struct(info) => Some(TypeInfo::Struct(info.as_ref().clone())),
        DataType::Array(info) => Some(TypeInfo::Array(info.as_ref().clone())),
        _ => None,
    }
}

/// Rebuild a `DataType` from its `TypeInfo` if the variant is parameterized.
pub fn data_type_from_info(code: u8, info: &TypeInfo) -> Option<DataType> {
    match (code, info) {
        (16, TypeInfo::List(e)) => Some(DataType::List(e.clone())),
        (17, TypeInfo::Map(v)) => Some(DataType::Map(v.clone())),
        (18, TypeInfo::Set(e)) => Some(DataType::Set(e.clone())),
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
    fn test_list_map_set_type_info_roundtrip() {
        for (info, code) in [
            (TypeInfo::list(DataType::Int), 16u8),
            (TypeInfo::map(DataType::Double), 17u8),
            (TypeInfo::set(DataType::String), 18u8),
        ] {
            let encoded = postcard::to_allocvec(&info).expect("encode type info");
            let decoded: TypeInfo = postcard::from_bytes(&encoded).expect("decode type info");
            assert_eq!(decoded, info);
            let rebuilt = data_type_from_info(code, &info)
                .unwrap_or_else(|| panic!("code {code} must rebuild from TypeInfo"));
            assert_eq!(
                type_info_of(&rebuilt),
                Some(info),
                "code {code} type must roundtrip through type_info_of"
            );
        }
    }

    #[test]
    fn test_data_type_from_info_mismatched_code() {
        let info = TypeInfo::array(DataType::Int, None);
        assert!(data_type_from_info(64, &info).is_none());
        // Container metadata must not decode under a different code.
        assert!(data_type_from_info(16, &info).is_none());
    }
}
