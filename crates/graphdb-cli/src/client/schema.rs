//! Schema DDL type roundtrip tests
//!
//! The `PropertyDef` DTO itself lives in `graphdb-wire` (single contract
//! source); this module keeps the core `DataType` wire-name roundtrip tests.

#[cfg(test)]
mod tests {
    use graphdb_core::core::types::DataType;
    use std::str::FromStr;

    /// Every core `DataType` a property can carry roundtrips through the
    /// wire format (Display output) and back via the core `FromStr` parser.
    #[test]
    fn test_wire_format_roundtrip() {
        use graphdb_core::core::types::{ArrayTypeInfo, StructTypeInfo};
        use std::sync::Arc;

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
            DataType::List(Box::new(DataType::Empty)),
            DataType::Map(Box::new(DataType::Empty)),
            DataType::Set(Box::new(DataType::Int)),
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
