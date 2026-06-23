//! API Core Layer – Business logic that is independent of the transport layer
//!
//! It provides core functions such as query execution, transaction management, and Schema operations.
//! It is reused by the embedded layer and the network service layer.

pub mod batch;
pub mod error;
pub mod query_api;
pub mod schema_api;
pub mod sync_api;
pub mod transaction_api;
pub mod types;
#[cfg(feature = "qdrant")]
pub mod vector_api;

pub use batch::{
    BatchConfig, BatchError, BatchItem, BatchItemType, BatchOperation, BatchOperationBuilder,
    BatchResult,
};
pub use error::{CoreError, CoreResult, ExtendedErrorCode};
pub use query_api::QueryApi;
pub use schema_api::SchemaApi;
pub use sync_api::SyncApi;
pub use transaction_api::TransactionApi;
pub use types::*;
#[cfg(feature = "qdrant")]
pub use vector_api::{VectorApi, VectorSearchResult};

// Re-export the statistical types from the core layer.
pub use crate::core::{
    ErrorInfo, ErrorSummary, ErrorType, MetricType, MetricValue, QueryMetrics, QueryPhase,
    QueryProfile, QueryStatus, StatsManager,
};
