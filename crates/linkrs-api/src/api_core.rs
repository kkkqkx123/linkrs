//! API Core Layer – Business logic that is independent of the transport layer
//!
//! It provides core functions such as query execution, transaction management, and Schema operations.
//! It is reused by the embedded layer and the network service layer.

pub mod bulk_insert;
pub mod cost_profile;
pub mod error;
pub mod query_api;
#[cfg(any(feature = "fulltext", feature = "vector"))]
pub mod rebuild_source;
pub mod schema_api;
pub mod session_command;
pub mod sync_api;
pub mod transaction_api;
pub mod types;
#[cfg(feature = "vector")]
pub mod vector_api;

pub use bulk_insert::{
    BulkInsertConfig, BulkInsertError, BulkInsertItem, BulkInsertItemType, BulkInsertOperation,
    BulkInsertOperationBuilder, BulkInsertResult,
};
pub use cost_profile::{cost_config_for_profile, resolve_space_cost_configs};
pub use error::{CoreError, CoreResult, ExtendedErrorCode};
pub use query_api::QueryApi;
pub use schema_api::SchemaApi;
pub use session_command::{
    classify_session_command, is_command_like, CommandParseError, SessionCommand,
};
pub use sync_api::SyncApi;
pub use transaction_api::TransactionApi;
pub use types::*;
#[cfg(feature = "vector")]
pub use vector_api::{VectorApi, VectorSearchResult};

// Re-export the statistical types from the metrics layer.
pub use linkrs_metrics::{
    ErrorInfo, ErrorSummary, ErrorType, MetricType, MetricValue, QueryMetrics, QueryPhase,
    QueryProfile, QueryStatus, StatsManager,
};
