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
pub mod property;
pub mod property_trait;
pub mod property_value;
pub mod query;
pub mod schema_change;
pub mod schema_trait;
pub mod space;
pub mod space_name_validation;
pub mod span;
pub mod storage_ids;
pub mod table_tracker;
pub mod tag;
pub mod transaction_config;
pub mod transaction_context;
pub mod undo;
pub mod user;
pub mod user_storage;
// Full-text search types
pub mod fulltext_query;
pub mod memory_estimation;

// C API type definitions (behind feature gate)
pub mod c_api;
pub mod dump_restore;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DataType {
    Empty,
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
    List,
    Map,
    Set,
    Geography,
    DataSet,
    FixedString(usize),
    VID,
    Blob,
    Timestamp,
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
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DataType::Empty => write!(f, "EMPTY"),
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
            DataType::List => write!(f, "LIST"),
            DataType::Map => write!(f, "MAP"),
            DataType::Set => write!(f, "SET"),
            DataType::Geography => write!(f, "GEOGRAPHY"),
            DataType::DataSet => write!(f, "DATASET"),
            DataType::FixedString(n) => write!(f, "FIXEDSTRING({})", n),
            DataType::VID => write!(f, "VID"),
            DataType::Blob => write!(f, "BLOB"),
            DataType::Timestamp => write!(f, "TIMESTAMP"),
            DataType::Vector => write!(f, "VECTOR"),
            DataType::VectorDense(n) => write!(f, "VECTOR_DENSE({})", n),
            DataType::VectorSparse(n) => write!(f, "VECTOR_SPARSE({})", n),
            DataType::Json => write!(f, "JSON"),
            DataType::JsonB => write!(f, "JSONB"),
            DataType::Uuid => write!(f, "UUID"),
            DataType::Interval => write!(f, "INTERVAL"),
        }
    }
}

impl DataType {
    pub fn as_u8(&self) -> u8 {
        match self {
            DataType::Empty => 0,
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
            DataType::List => 16,
            DataType::Map => 17,
            DataType::Set => 18,
            DataType::Geography => 19,
            DataType::DataSet => 20,
            DataType::FixedString(_) => 21,
            DataType::VID => 22,
            DataType::Blob => 23,
            DataType::Timestamp => 24,
            DataType::Vector => 25,
            DataType::VectorDense(_) => 26,
            DataType::VectorSparse(_) => 27,
            DataType::Json => 28,
            DataType::JsonB => 29,
            DataType::Uuid => 30,
            DataType::Interval => 31,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => DataType::Empty,
            1 => DataType::Null,
            2 => DataType::Bool,
            3 => DataType::SmallInt,
            4 => DataType::Int,
            5 => DataType::BigInt,
            6 => DataType::Float,
            7 => DataType::Double,
            8 => DataType::Decimal128,
            9 => DataType::String,
            10 => DataType::Date,
            11 => DataType::Time,
            12 => DataType::DateTime,
            13 => DataType::Vertex,
            14 => DataType::Edge,
            15 => DataType::Path,
            16 => DataType::List,
            17 => DataType::Map,
            18 => DataType::Set,
            19 => DataType::Geography,
            20 => DataType::DataSet,
            21 => DataType::FixedString(0),
            22 => DataType::VID,
            23 => DataType::Blob,
            24 => DataType::Timestamp,
            25 => DataType::Vector,
            26 => DataType::VectorDense(0),
            27 => DataType::VectorSparse(0),
            28 => DataType::Json,
            29 => DataType::JsonB,
            30 => DataType::Uuid,
            31 => DataType::Interval,
            _ => DataType::Empty,
        }
    }
}

// Exporting Base Schema Types from Atomic Modules
pub use self::edge::{EdgeStrategy, EdgeTypeInfo};
pub use self::index::{Index, IndexConfig, IndexField, IndexStatus, IndexType};
// Export full-text index types
pub use self::index::{
    BM25IndexConfig, FulltextEngineType, FulltextIndexField, FulltextIndexOptions,
};
// Export full-text query types
pub use self::fulltext_query::{
    FieldQuery, FulltextQuery, FulltextQueryOptions, FulltextSearchResult, HighlightOptions,
    QueryExplanation, SearchResultEntry, ShardFailure, ShardsInfo, SortField, SortMissing,
    SortOrder,
};
pub use self::property::PropertyDef;
pub use self::property_value::PropertyValue;
pub use self::space::{EngineType, IsolationLevel, SpaceInfo, SpaceStatus, SpaceSummary};
pub use self::tag::TagInfo;

// Exporting version types from metadata_version
pub use self::metadata_version::{MetadataVersion, SchemaHistory, SchemaVersion};

// Exporting types from split submodules
pub use self::cluster::ClusterInfo;
pub use self::compact::{CompactConfig, CompactError, CompactResult, CompactStats, CompactTarget, CompactionStrategy, AdaptiveCompactionConfig};
pub use self::data_modification::{
    InsertEdgeInfo, InsertVertexInfo, UpdateInfo, UpdateOp, UpdateTarget,
};
pub use self::import_export::{ExportFormat, SchemaExportConfig, SchemaImportResult};
pub use self::schema_change::{
    AlterTargetType, FieldChangeType, SchemaAlterOperation, SchemaChange, SchemaChangeType,
    SchemaFieldChange,
};
pub use self::space::CharsetInfo;
pub use self::user::{PasswordInfo, UserAlterInfo, UserInfo};

pub use self::expr::{ContextualExpression, Expression, ExpressionMeta, SerializableExpression};
pub use self::graph_schema::{
    EdgeDirection, EdgeTypeRef, GraphTypeInference, JoinType, OrderDirection, PathInfo,
    PropertyType, VertexType,
};
pub use self::operators::{AggregateFunction, BinaryOperator, UnaryOperator};
pub use self::query::{
    ExecutionMode, PlanType, QueryHint, QueryOptions, QueryStats, QueryStatus, QueryType,
};
pub use self::span::{Position, Span, ToSpan};

// Export storage identifier types for cross-module usage
pub use self::storage_ids::{
    ColumnId, EdgeDeletionContext, EdgeDeletionContextParams, EdgeId, EdgeIdentifier, EdgeKey,
    EdgeLocation, EdgeOperationContext, EdgePropertyUpdateContext, LabelId, Timestamp,
    TransactionId, VertexId, VertexIdentifier, INVALID_EDGE_ID, INVALID_TIMESTAMP, MAX_TIMESTAMP,
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
    pub expression: crate::core::types::expr::contextual::ContextualExpression,
    pub alias: String,
    pub is_matched: bool,
}

impl YieldColumn {
    pub fn new(
        expression: crate::core::types::expr::contextual::ContextualExpression,
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
