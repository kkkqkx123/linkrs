pub mod cluster;
pub mod compact;
pub mod data_modification;
pub mod data_set;
pub mod edge;
pub mod expr;
pub mod graph_schema;
pub mod import_export;
pub mod index;
pub mod metadata_version;
pub mod operators;
pub mod parse;
pub mod property;
pub mod property_trait;
pub mod query;
pub mod schema_change;
pub mod schema_trait;
pub mod semantic;
pub mod space;
pub mod space_name_validation;
pub mod span;
pub mod storage_ids;
pub mod sync_protocol;
pub mod table_tracker;
pub mod tag;
pub mod transaction_config;
pub mod transaction_context;
pub mod undo;
pub mod user;
pub mod user_storage;
pub mod version;
// Full-text search types
pub mod fulltext_query;
pub mod memory_estimation;

// C API type definitions (behind feature gate)
pub mod c_api;

use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub mod type_info;

pub use parse::ParseDataTypeError;
pub use type_info::{data_type_from_info, type_info_of, ArrayTypeInfo, StructTypeInfo, TypeInfo};

/// Error decoding a `DataType` from its compact byte code.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TypeCodecError {
    #[error("unknown data type code {0}")]
    UnknownTypeCode(u8),
    #[error("reserved data type code {0}")]
    ReservedTypeCode(u8),
    #[error("parameterized data type code {0} requires type metadata")]
    ParameterizedTypeCode(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataType {
    Empty,
    /// Type unknown at parse/deduction time; resolved by the binder against
    /// schema or binding scope. Distinct from `Empty` (which maps the explicit
    /// `Value::Empty`), so "unknown type" and "empty value" never conflate.
    Unknown,
    Null,
    Bool,
    // Integer types: simplified to 3 types (aligned with PostgreSQL)
    SmallInt, // i16
    Int,      // i32
    BigInt,   // i64
    // Floating point types: 2 types (standard practice)
    Float,  // f32
    Double, // f64
    Decimal128,
    String,
    Date,
    Time,
    DateTime,
    Vertex,
    Edge,
    Path,
    /// Variable-length list. Carries the homogeneous element type; an `Empty`
    /// element type marks an untyped container (the pre-parameterization form
    /// still accepted by DDL as bare `LIST`).
    List(Box<DataType>),
    /// String-keyed map. Carries the shared value type; an `Empty` value type
    /// marks an untyped container (bare `MAP` in DDL).
    Map(Box<DataType>),
    /// Set of unique elements. Carries the homogeneous element type; an
    /// `Empty` element type marks an untyped container (bare `SET` in DDL).
    Set(Box<DataType>),
    Geography,
    DataSet,
    FixedString(usize),
    Blob,
    Vector,
    VectorDense(usize),
    VectorSparse(usize),

    /// JSON text type
    Json,
    /// JSONB binary type
    JsonB,
    /// UUID type
    Uuid,
    /// Interval type
    Interval,

    /// STRUCT: named-field composite type with metadata.
    Struct(Arc<StructTypeInfo>),
    /// ARRAY: element-homogeneous composite type with metadata.
    Array(Arc<ArrayTypeInfo>),
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataType::Empty => write!(f, "EMPTY"),
            DataType::Unknown => write!(f, "UNKNOWN"),
            DataType::Null => write!(f, "NULL"),
            DataType::Bool => write!(f, "BOOL"),
            DataType::SmallInt => write!(f, "SMALLINT"),
            DataType::Int => write!(f, "INT"),
            DataType::BigInt => write!(f, "BIGINT"),
            DataType::Float => write!(f, "FLOAT"),
            DataType::Double => write!(f, "DOUBLE"),
            DataType::Decimal128 => write!(f, "DECIMAL128"),
            DataType::String => write!(f, "STRING"),
            DataType::Date => write!(f, "DATE"),
            DataType::Time => write!(f, "TIME"),
            DataType::DateTime => write!(f, "DATETIME"),
            DataType::Vertex => write!(f, "VERTEX"),
            DataType::Edge => write!(f, "EDGE"),
            DataType::Path => write!(f, "PATH"),
            DataType::List(element) => {
                // Untyped containers (element `Empty`) keep the legacy bare
                // spelling so `Display` roundtrips through `from_str`.
                if element.as_ref() == &DataType::Empty {
                    write!(f, "LIST")
                } else {
                    write!(f, "LIST<{}>", element)
                }
            }
            DataType::Map(value) => {
                if value.as_ref() == &DataType::Empty {
                    write!(f, "MAP")
                } else {
                    write!(f, "MAP<{}>", value)
                }
            }
            DataType::Set(element) => {
                if element.as_ref() == &DataType::Empty {
                    write!(f, "SET")
                } else {
                    write!(f, "SET<{}>", element)
                }
            }
            DataType::Geography => write!(f, "GEOGRAPHY"),
            DataType::DataSet => write!(f, "DATASET"),
            DataType::FixedString(n) => write!(f, "FIXEDSTRING({})", n),
            DataType::Blob => write!(f, "BLOB"),
            DataType::Vector => write!(f, "VECTOR"),
            DataType::VectorDense(n) => write!(f, "VECTOR_DENSE({})", n),
            DataType::VectorSparse(n) => write!(f, "VECTOR_SPARSE({})", n),
            DataType::Json => write!(f, "JSON"),
            DataType::JsonB => write!(f, "JSONB"),
            DataType::Uuid => write!(f, "UUID"),
            DataType::Interval => write!(f, "INTERVAL"),
            DataType::Struct(info) => {
                write!(f, "STRUCT<")?;
                for (i, (name, field_type)) in info.fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{} {}", name, field_type)?;
                }
                write!(f, ">")
            }
            DataType::Array(info) => {
                write!(f, "ARRAY<{}>", info.element)?;
                if let Some(len) = info.len {
                    write!(f, "({})", len)?;
                }
                Ok(())
            }
        }
    }
}

impl DataType {
    /// Compact byte code of the data type.
    ///
    /// Codes 0-31 are fixed. Codes 22 and 24 are reserved (previously used by
    /// the removed `VID` and `Timestamp` types) and must not be reused.
    /// New types are allocated from code 64 onwards; code 32 is additionally
    /// assigned to `Unknown` (a binding-time sentinel that never reaches
    /// storage serialization). Codes 16/17/18 (`List`/`Map`/`Set`) are
    /// parameterized: decoding a bare code requires the accompanying `TypeInfo`
    /// metadata, exactly like `Struct`/`Array`.
    pub fn as_u8(&self) -> u8 {
        match self {
            DataType::Empty => 0,
            DataType::Unknown => 32,
            DataType::Null => 1,
            DataType::Bool => 2,
            DataType::SmallInt => 3,
            DataType::Int => 4,
            DataType::BigInt => 5,
            DataType::Float => 6,
            DataType::Double => 7,
            DataType::Decimal128 => 8,
            DataType::String => 9,
            DataType::Date => 10,
            DataType::Time => 11,
            DataType::DateTime => 12,
            DataType::Vertex => 13,
            DataType::Edge => 14,
            DataType::Path => 15,
            DataType::List(_) => 16,
            DataType::Map(_) => 17,
            DataType::Set(_) => 18,
            DataType::Geography => 19,
            DataType::DataSet => 20,
            DataType::FixedString(_) => 21,
            DataType::Blob => 23,
            DataType::Vector => 25,
            DataType::VectorDense(_) => 26,
            DataType::VectorSparse(_) => 27,
            DataType::Json => 28,
            DataType::JsonB => 29,
            DataType::Uuid => 30,
            DataType::Interval => 31,
            DataType::Struct(_) => 64,
            DataType::Array(_) => 65,
        }
    }

    /// Decode a data type from its compact byte code.
    ///
    /// Returns an error for unknown codes, for the reserved codes 22/24
    /// (previously `VID`/`Timestamp`), for the parameterized codes 16/17/18
    /// and >= 64 (which need `TypeInfo` metadata), and for codes >= 64 that
    /// were not yet allocated by a newer version. Unknown codes never silently
    /// decode to `Empty`; forward compatibility failures surface explicitly.
    pub fn from_u8(value: u8) -> Result<DataType, TypeCodecError> {
        match value {
            0 => Ok(DataType::Empty),
            32 => Ok(DataType::Unknown),
            1 => Ok(DataType::Null),
            2 => Ok(DataType::Bool),
            3 => Ok(DataType::SmallInt),
            4 => Ok(DataType::Int),
            5 => Ok(DataType::BigInt),
            6 => Ok(DataType::Float),
            7 => Ok(DataType::Double),
            8 => Ok(DataType::Decimal128),
            9 => Ok(DataType::String),
            10 => Ok(DataType::Date),
            11 => Ok(DataType::Time),
            12 => Ok(DataType::DateTime),
            13 => Ok(DataType::Vertex),
            14 => Ok(DataType::Edge),
            15 => Ok(DataType::Path),
            16 => Err(TypeCodecError::ParameterizedTypeCode(16)),
            17 => Err(TypeCodecError::ParameterizedTypeCode(17)),
            18 => Err(TypeCodecError::ParameterizedTypeCode(18)),
            19 => Ok(DataType::Geography),
            20 => Ok(DataType::DataSet),
            21 => Ok(DataType::FixedString(0)),
            22 => Err(TypeCodecError::ReservedTypeCode(22)),
            23 => Ok(DataType::Blob),
            24 => Err(TypeCodecError::ReservedTypeCode(24)),
            25 => Ok(DataType::Vector),
            26 => Ok(DataType::VectorDense(0)),
            27 => Ok(DataType::VectorSparse(0)),
            28 => Ok(DataType::Json),
            29 => Ok(DataType::JsonB),
            30 => Ok(DataType::Uuid),
            31 => Ok(DataType::Interval),
            // Parameterized types need TypeInfo metadata to decode; the bare
            // code alone is not sufficient. The caller must read the metadata
            // block and rebuild the DataType.
            64 => Err(TypeCodecError::ParameterizedTypeCode(64)),
            65 => Err(TypeCodecError::ParameterizedTypeCode(65)),
            _ => Err(TypeCodecError::UnknownTypeCode(value)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `DataType` variant with its compact byte code.
    ///
    /// Codes 22 and 24 are intentionally absent: they were previously used by
    /// the removed `VID`/`Timestamp` types and are now reserved.
    /// Parameterized types (`List`/`Map`/`Set`/`Struct`/`Array`) decode to
    /// their codes (16/17/18/64/65) only together with `TypeInfo` metadata.
    fn all_data_types() -> Vec<DataType> {
        vec![
            DataType::Empty,
            DataType::Unknown,
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
            DataType::List(Box::new(DataType::String)),
            DataType::Map(Box::new(DataType::Int)),
            DataType::Set(Box::new(DataType::String)),
            DataType::Geography,
            DataType::DataSet,
            DataType::FixedString(0),
            DataType::Blob,
            DataType::Vector,
            DataType::VectorDense(0),
            DataType::VectorSparse(0),
            DataType::Json,
            DataType::JsonB,
            DataType::Uuid,
            DataType::Interval,
            DataType::Struct(Arc::new(StructTypeInfo::new(vec![(
                "city".to_string(),
                DataType::String,
            )]))),
            DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::Double, Some(3)))),
        ]
    }

    #[test]
    fn test_as_u8_codes_are_stable_within_range() {
        // Assigned codes stay in 0-31, in the 64+ parameterized range, or are
        // the dedicated `Unknown` code 32.
        for data_type in all_data_types() {
            let code = data_type.as_u8();
            assert!(
                code <= 31 || code == 32 || (64..=65).contains(&code),
                "assigned code {code} for {data_type:?} must stay within 0-32 or 64-65"
            );
        }
    }

    #[test]
    fn test_from_u8_roundtrip_for_all_variants() {
        for data_type in all_data_types() {
            let code = data_type.as_u8();
            match DataType::from_u8(code) {
                Ok(decoded) => {
                    assert_eq!(
                        decoded, data_type,
                        "roundtrip mismatch for {data_type:?} (code {code})"
                    );
                }
                Err(TypeCodecError::ParameterizedTypeCode(c)) => {
                    assert_eq!(c, code, "parameterized code {code} must roundtrip");
                }
                Err(e) => panic!("code {code} for {data_type:?} must decode: {e}"),
            }
        }
    }

    #[test]
    fn test_parameterized_codes_require_metadata() {
        // Codes 16/17/18 (List/Map/Set) and 64/65 (Struct/Array) are known but
        // parameterized: decoding the bare code must fail with the explicit
        // `ParameterizedTypeCode` error, never silently yield a
        // parameter-free type.
        for code in [16u8, 17, 18, 64, 65] {
            assert_eq!(
                DataType::from_u8(code),
                Err(TypeCodecError::ParameterizedTypeCode(code)),
                "code {code} must be parameterized"
            );
        }
        // Unknown codes in the 64+ range still error as unknown.
        for code in [66u8, 100, 255] {
            assert_eq!(
                DataType::from_u8(code),
                Err(TypeCodecError::UnknownTypeCode(code)),
                "unassigned code {code} must error as unknown"
            );
        }
    }

    #[test]
    fn test_from_u8_rejects_reserved_codes() {
        // 22 (former `VID`) and 24 (former `Timestamp`) must never silently
        // decode into a valid type.
        assert_eq!(
            DataType::from_u8(22),
            Err(TypeCodecError::ReservedTypeCode(22))
        );
        assert_eq!(
            DataType::from_u8(24),
            Err(TypeCodecError::ReservedTypeCode(24))
        );
    }

    #[test]
    fn test_from_u8_rejects_unknown_codes_instead_of_empty() {
        // The reserved expansion range (64+) and any unassigned code must fail
        // loudly instead of silently degrading to `Empty`.
        for code in [33u8, 63, 66, 100, 128, 255] {
            assert_eq!(
                DataType::from_u8(code),
                Err(TypeCodecError::UnknownTypeCode(code)),
                "unassigned code {code} must error"
            );
        }
        // Parameterized codes fail with a distinct, explicit error.
        for code in [16u8, 17, 18, 64, 65] {
            assert_eq!(
                DataType::from_u8(code),
                Err(TypeCodecError::ParameterizedTypeCode(code))
            );
        }
    }

    #[test]
    fn test_from_u8_never_yields_empty_for_nonzero_code() {
        // Regression guard: unknown codes must not collapse into `Empty`.
        for code in 1..=255 {
            if let Ok(DataType::Empty) = DataType::from_u8(code) {
                panic!("code {code} must not decode to Empty");
            }
        }
        // Code 32 is the dedicated `Unknown` sentinel.
        assert_eq!(DataType::from_u8(32), Ok(DataType::Unknown));
    }
}

// Exporting Base Schema Types from Atomic Modules
pub use self::edge::{EdgeStrategy, EdgeTypeInfo};
pub use self::index::{Index, IndexConfig, IndexField, IndexStatus, IndexType};
// Export full-text index types
pub use self::index::{
    BM25IndexConfig, ConsistencyState, FulltextEngineType, FulltextIndexField,
    FulltextIndexOptions, IndexStats, SearchStats,
};
// Export full-text query types
pub use self::fulltext_query::{
    FieldQuery, FulltextQuery, FulltextQueryOptions, FulltextSearchResult, HighlightOptions,
    QueryExplanation, SearchResultEntry, ShardFailure, ShardsInfo, SortField, SortMissing,
    SortOrder,
};
pub use self::property::PropertyDef;
pub use self::space::{EngineType, IsolationLevel, SpaceInfo, SpaceStatus, SpaceSummary};
pub use self::tag::TagInfo;

// Exporting version types from metadata_version
pub use self::metadata_version::{MetadataVersion, SchemaHistory, SchemaVersion};
// Exporting storage version type
pub use self::version::StorageVersion;

// Exporting types from split submodules
pub use self::cluster::ClusterInfo;
pub use self::compact::{
    AdaptiveCompactionConfig, AutoCompactConfig, CompactConfig, CompactError, CompactResult,
    CompactStats, CompactTarget, CompactionStrategy,
};
pub use self::data_modification::{
    InsertEdgeInfo, InsertVertexInfo, UpdateInfo, UpdateOp, UpdateTarget,
};
pub use self::import_export::{ExportFormat, SchemaExportConfig, SchemaImportResult};
pub use self::schema_change::{
    AlterTargetType, FieldChangeType, SchemaAlterOperation, SchemaChange, SchemaChangeType,
    SchemaFieldChange,
};
pub use self::space::CharsetInfo;
pub use self::user::{set_bcrypt_cost, PasswordInfo, UserAlterInfo, UserInfo};

pub use self::expr::{ContextualExpression, Expression, ExpressionMeta, SerializableExpression};
pub use self::graph_schema::{
    EdgeDirection, EdgeTypeRef, GraphTypeInference, JoinType, OrderDirection, PathInfo,
    PropertyType, VertexType,
};
pub use self::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
pub use self::query::{
    ExecutionMode, PlanType, QueryHint, QueryOptions, QueryStats, QueryStatus, QueryType,
};
pub use self::semantic::{AliasType, ColumnDef, ValueType};
pub use self::span::{Position, Span, ToSpan};

// Export storage identifier types for cross-module usage
pub use self::storage_ids::{
    ColumnId, EdgeDeletionContext, EdgeDeletionContextParams, EdgeId, EdgeIdentifier, EdgeKey,
    EdgeLocation, EdgeOperationContext, EdgePropertyUpdateContext, LabelId, Timestamp,
    TransactionId, VertexId, VertexIdentifier, INVALID_EDGE_ID, INVALID_TIMESTAMP, MAX_TIMESTAMP,
};
pub use self::sync_protocol::{
    CommitLsn, IdempotencyKey, IndexGeneration, LeaseEpoch, OrderingKey, SnapshotTimestamp,
    TargetId,
};
pub use self::table_tracker::{TableId, TableTracker, TableTrackerConfig, TableType};
pub use self::transaction_config::{DurabilityLevel, TransactionIsolationLevel};
pub use self::transaction_context::TransactionContextInfo;
pub use self::undo::{UndoLogError, UndoLogResult, UndoTarget};

pub use EdgeTypeInfo as EdgeTypeSchema;

/// YIELD column definition
///
/// Indicates an output column in the YIELD clause
#[derive(Debug, Clone)]
pub struct YieldColumn {
    pub expression: crate::types::expr::contextual::ContextualExpression,
    pub alias: String,
    pub is_matched: bool,
}

impl YieldColumn {
    pub fn new(
        expression: crate::types::expr::contextual::ContextualExpression,
        alias: String,
    ) -> Self {
        Self {
            expression,
            alias,
            is_matched: false,
        }
    }

    pub fn with_matched(mut self, is_matched: bool) -> Self {
        self.is_matched = is_matched;
        self
    }

    /// Get column name (alias)
    pub fn name(&self) -> &str {
        &self.alias
    }
}

pub use data_set::DataSet;
pub use user_storage::UserStorage;
