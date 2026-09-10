// Query module for the graph database
//
// This module provides the complete query processing pipeline including:
// - Parsing query strings into AST
// - Planning and optimizing execution plans
// - Executing queries against the storage engine
// - Managing query contexts and validation

// Sub-modules
pub mod binder;
pub mod cache;
pub mod context;
pub mod cte;
pub mod executor;
pub mod extensions;
pub mod metadata;
pub mod optimizer;
pub mod parser;
pub mod pipeline;
pub mod planning;
pub mod query_core;
pub mod query_manager;
pub mod session_events;

// Re-export DataSet for convenience
pub use graphdb_core::DataSet;
// Re-export error types from core module
pub use graphdb_core::{DBResult, QueryError};
// Re-export execution result from executor module
pub use executor::base::ExecutionResult;
// Re-export QueryPipelineManager
pub use pipeline::QueryPipelineManager;
// Re-export context types from context module
pub use context::{QueryContext, QueryContextBuilder, QueryRequestContext};
// Re-export QueryManager
pub use query_manager::{
    QueryInfo, QueryManager, QueryProgress, QueryProgressCallback, QueryStatus,
};
// Re-export pipeline extensions.
//
// Unstable experimental API: the extension surface may evolve while the
// pipeline integration matures.
pub use extensions::{
    BinderExtension, BinderExtensionContext, ExtensionRegistry, MapperExtension, ParserExtension,
    PlannerExtension,
};
/// Unstable alias for the experimental pipeline-extension surface.
///
/// New code should import from here instead of `extensions` so future
/// stabilization (or removal) has one obvious place to change. The alias
/// re-exports everything in [`extensions`]; both paths refer to the same
/// types.
pub mod experimental_pipeline_extensions {
    pub use super::extensions::*;
}
pub use session_events::{SessionEvent, SessionEventCallback};
// Re-export OptimizerEngine
pub use optimizer::OptimizerEngine;

pub mod storage {
    pub use graphdb_storage::*;

    #[cfg(test)]
    pub use graphdb_storage::MockStorage;
}
