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
pub mod utils;

pub mod wal;



// Error and result types
pub use error::{
    DBError, DBResult, ErrorCategory, GraphDBResult, ManagerError, ManagerResult,
    PlanNodeVisitError, QueryError, QueryResult, StorageError, StorageResult,
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

pub use types::graph_schema::EdgeDirection;

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

pub use types::dump_restore::{CompressionType, DumpConfig, DumpError, DumpFormat, DumpMetadata, RestoreConfig, RestoreError, RestoreStats, SpaceDumpInfo};
