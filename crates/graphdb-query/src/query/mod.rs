// Query module for the graph database
//
// This module provides the complete query processing pipeline including:
// - Parsing query strings into AST
// - Planning and optimizing execution plans
// - Executing queries against the storage engine
// - Managing query contexts and validation

// Sub-modules
pub mod cache;
pub mod context;
pub mod core;
pub mod data_set;
pub mod executor;
pub mod metadata;
pub mod optimizer;
pub mod parser;
pub mod planning;
pub mod query_manager;
pub mod query_pipeline_manager;
pub mod validator;

// Re-export DataSet for convenience
pub use data_set::DataSet;
// Re-export error types from core module
pub use crate::core::{DBResult, QueryError};
// Re-export execution result from executor module
pub use executor::base::ExecutionResult;
// Re-export QueryPipelineManager
pub use query_pipeline_manager::QueryPipelineManager;
// Re-export context types from context module
pub use context::{QueryContext, QueryContextBuilder, QueryExecutionManager, QueryRequestContext};
// Re-export QueryManager
pub use query_manager::{QueryInfo, QueryManager, QueryStats, QueryStatus};
// Re-export OptimizerEngine
pub use optimizer::OptimizerEngine;
