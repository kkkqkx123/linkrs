//! Query execution context module
//!
//! This module provides all context types used during query execution:
//! - [`QueryContext`]: Main context for query lifecycle management
//! - [`QueryRequestContext`]: Request-level context (session, parameters)
//! - [`QueryContextBuilder`]: Builder pattern for constructing QueryContext
//!
//! # Architecture
//!
//! The context system follows a composite pattern:
//! ```text
//! QueryContext
//! ├── rctx: QueryRequestContext  (request info, parameters)
//! ├── cancel_token: CancelToken  (request-scoped cancellation, shared with runtime/registry)
//! ├── id_gen: IdGenerator  (unique ID generation)
//! ├── space_info: Option<SpaceInfo>  (current graph space)
//! └── charset_info: Option<CharsetInfo>  (character set)
//! ```

pub mod query_context;
pub mod query_context_builder;
pub mod query_request_context;

pub use query_context::QueryContext;
pub use query_context_builder::QueryContextBuilder;
pub use query_request_context::QueryRequestContext;
