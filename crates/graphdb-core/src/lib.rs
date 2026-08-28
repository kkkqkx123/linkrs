pub mod error;
pub mod metadata;
pub mod npath;
pub mod session_stats;
pub mod type_system;
pub mod value;
pub mod vertex_edge_path;

// New sub-modules
pub mod permission;
pub mod stats;
pub mod types;
pub mod vector;
pub mod wal;

// Utility modules
pub mod arena;
pub mod bloom_filter;
pub mod id_gen;
pub mod null_bitmap;
pub mod value_conversion;

// Error and result types
pub use error::{
    DBError, DBResult, ErrorCategory, ManagerError, ManagerResult, PlanNodeVisitError, QueryError,
    QueryResult, StorageError, StorageResult,
};

// External error code
pub use error::{ErrorCode, PublicError, ToPublicError};

// Core data types
pub use npath::{NPath, NPathEdgeIter, NPathIter, NPathVertexIter};
pub use value::*;
pub use vertex_edge_path::{Edge, Path, Step, Tag, Vertex};

// Expression system type
pub use types::expr::Expression;
pub use types::DataType;

// Type metadata and codec errors
pub use types::{
    data_type_from_info, type_info_of, ArrayTypeInfo, StructTypeInfo, TypeCodecError, TypeInfo,
};

pub use types::graph_schema::EdgeDirection;

pub use types::index::{ConsistencyState, IndexStats, SearchStats};

pub use types::operators::{AggregateFunction, BinaryOperator, UnaryOperator};

pub use types::DataSet;
pub use types::UserStorage;
pub use types::YieldColumn;

// Other core types
pub use type_system::TypeUtils;

// Permission type
pub use permission::{Permission, RoleType};

// Statistical type
pub use stats::{
    ErrorInfo, ErrorSummary, ErrorType, MetricType, MetricValue, QueryMetrics, QueryPhase,
    QueryProfile, QueryStatus, StatsManager,
};

// Session statistics type
pub use session_stats::SessionStatistics;

// Utility re-exports
pub use arena::{Arena, ArenaPool, ArenaStringBuilder, ArenaTokenizer, ArenaVec};
pub use bloom_filter::{BloomFilter, ScalableBloomFilter};
pub use id_gen::{generate_id, IdGenerator};
pub use null_bitmap::NullBitmap;
