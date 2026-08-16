//! Schema DDL types

use graphdb_core::core::types::DataType;

/// Property definition for schema creation
///
/// Wire DTO: the type name is serialized via the core `DataType` `Display`
/// output (`BOOL`/`INT`/`FIXEDSTRING(8)`/`VECTOR_DENSE(3)` ...), which the
/// server parses back through the same `FromStr` source of truth.
#[derive(Debug, Clone)]
pub struct PropertyDef {
    pub name: String,
    pub data_type: DataType,
    pub nullable: bool,
}

impl PropertyDef {
    pub fn new(name: impl Into<String>, data_type: DataType) -> Self {
        Self {
            name: name.into(),
            data_type,
            nullable: true,
        }
    }

    pub fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::core::types::{ArrayTypeInfo, StructTypeInfo};
    use std::str::FromStr;
    use std::sync::Arc;

    /// Every core `DataType` a `PropertyDef` can carry roundtrips through the
    /// wire format (Display output) and back via the core `FromStr` parser.
    #[test]
    fn test_wire_format_roundtrip() {
        let types = vec![
            DataType::Empty,
            DataType::Null,
            DataType::Bool,
            DataType::SmallInt,
            DataType::Int,
            DataType::BigInt,
            DataType::Float,
            DataType::Double,
            DataType::Decimal128,
            DataType::String,
            DataType::Date,
            DataType::Time,
            DataType::DateTime,
            DataType::Vertex,
            DataType::Edge,
            DataType::Path,
            DataType::List,
            DataType::Map,
            DataType::Set,
            DataType::Geography,
            DataType::DataSet,
            DataType::FixedString(8),
            DataType::Blob,
            DataType::Vector,
            DataType::VectorDense(3),
            DataType::VectorSparse(3),
            DataType::Json,
            DataType::JsonB,
            DataType::Uuid,
            DataType::Interval,
            DataType::Struct(Arc::new(StructTypeInfo::new(vec![(
                "city".to_string(),
                DataType::String,
            )]))),
            DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::Double, Some(3)))),
        ];
        for data_type in types {
            let wire = data_type.to_string();
            let parsed = DataType::from_str(&wire)
                .unwrap_or_else(|e| panic!("cannot parse wire type '{wire}': {e}"));
            assert_eq!(parsed, data_type, "roundtrip mismatch for '{wire}'");
        }
    }

    /// The removed local `Timestamp` variant must not resurface: TIMESTAMP
    /// normalizes to DATETIME, so the wire format never carries a distinct
    /// `TIMESTAMP` value.
    #[test]
    fn test_timestamp_is_not_a_distinct_type() {
        assert_eq!(DataType::from_str("TIMESTAMP").unwrap(), DataType::DateTime);
        assert_eq!(DataType::DateTime.to_string(), "DATETIME");
    }
}
