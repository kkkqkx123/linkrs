//! Query context
//!
//! Manage the context information throughout the entire lifecycle of queries, from parsing and validation to planning and execution.
//!
//! ## Creation
//!
//! Use [`QueryContext::new`] for simple cases or [`QueryContext::builder`] for complex configuration:
//!
//! ```rust,ignore
//! use graphdb::query::context::{QueryContext, QueryRequestContext};
//! use std::sync::Arc;
//!
//! // Simple creation
//! let rctx = Arc::new(QueryRequestContext::new("MATCH (n) RETURN n".to_string()));
//! let ctx = QueryContext::new(rctx);
//!
//! // With builder (for complex configuration)
//! let ctx = QueryContext::builder(rctx)
//!     .with_space_info(space_info)
//!     .with_arena()
//!     .build();
//! ```
//!
//! ## Arena Allocation
//!
//! For high-performance query execution with many temporary allocations,
//! enable arena allocation via the builder's `with_arena()` method. This is beneficial for:
//!
//! - Complex queries with many intermediate results
//! - Expression evaluation with temporary values
//! - Graph traversal with path accumulation

use std::sync::Arc;

use crate::core::types::{CharsetInfo, SpaceInfo, Timestamp};
use crate::executor::streaming::query_registry::CancelToken;
use crate::executor::streaming::transaction_scope::CancelReason;
use crate::utils::{Arena, IdGenerator};

use super::QueryRequestContext;

/// Query context
///
/// The context for each query request is created whenever the query request is received by the query engine.
/// This context object is visible to the parser, planner, optimizer, and executor.
///
/// # Responsibilities
///
/// - Query request context (session information, request parameters)
/// - Request-scoped cancellation token (shared with the execution runtime and
///   the process-level query registry — one token for all cancellation)
/// - ID generation for query execution
/// - Space information management (space info, character set)
/// - Optional arena allocator for high-performance temporary allocations
///
/// # Creation
///
/// Use [`QueryContext::new`] for simple cases or [`QueryContext::builder`] for complex configuration.
pub struct QueryContext {
    /// Query request context
    rctx: Arc<QueryRequestContext>,
    /// Request-scoped cancellation token.
    ///
    /// The execution pipeline threads this token into the query registry and
    /// the execution runtime (`instantiate_plan`), so `mark_killed`, KILL
    /// QUERY, and runtime cancel all flip the same underlying state.
    cancel_token: CancelToken,
    /// ID Generator for query execution
    id_gen: IdGenerator,
    /// Current space information
    space_info: Option<SpaceInfo>,
    /// Character set information
    charset_info: Option<Box<CharsetInfo>>,
    /// MVCC snapshot timestamp for consistent reads within explicit
    /// transactions. `None` = auto-commit single statement (current-version
    /// reads).
    snapshot_ts: Option<Timestamp>,
    /// Transaction isolation level injected by the API layer for queries
    /// running inside an explicit transaction. `None` = auto-commit
    /// statement-level snapshot semantics. Execution-time knob: the runtime
    /// and operators can consult it instead of only the storage layer.
    isolation_level: Option<crate::core::types::TransactionIsolationLevel>,
    /// Optional arena allocator for temporary allocations during query execution
    arena: Option<Arena>,
}

/// Builder-supplied context parameters, grouped so the internal constructor
/// stays within a manageable arity.
pub(super) struct ContextParams {
    pub cancel_token: CancelToken,
    pub id_gen: IdGenerator,
    pub space_info: Option<SpaceInfo>,
    pub charset_info: Option<Box<CharsetInfo>>,
    pub snapshot_ts: Option<Timestamp>,
    pub isolation_level: Option<crate::core::types::TransactionIsolationLevel>,
    pub arena: Option<Arena>,
}

impl QueryContext {
    /// Create a new query context with default configuration.
    ///
    /// For complex configuration (arena allocation, custom ID generator, etc.),
    /// use [`QueryContext::builder`] instead.
    pub fn new(rctx: Arc<QueryRequestContext>) -> Self {
        Self {
            rctx,
            cancel_token: CancelToken::new(),
            id_gen: IdGenerator::new(0),
            space_info: None,
            charset_info: None,
            snapshot_ts: None,
            isolation_level: None,
            arena: None,
        }
    }

    /// Create a builder for complex configuration.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let ctx = QueryContext::builder(rctx)
    ///     .with_space_info(space_info)
    ///     .with_charset_info(charset_info)
    ///     .with_arena()
    ///     .build();
    /// ```
    pub fn builder(rctx: Arc<QueryRequestContext>) -> super::QueryContextBuilder {
        super::QueryContextBuilder::new(rctx)
    }

    /// Internal constructor for QueryContextBuilder.
    /// Only visible within the query::context module.
    pub(super) fn from_builder(rctx: Arc<QueryRequestContext>, params: ContextParams) -> Self {
        Self {
            rctx,
            cancel_token: params.cancel_token,
            id_gen: params.id_gen,
            space_info: params.space_info,
            charset_info: params.charset_info,
            snapshot_ts: params.snapshot_ts,
            isolation_level: params.isolation_level,
            arena: params.arena,
        }
    }

    /// Obtain the context of the query request.
    pub fn request_context(&self) -> &QueryRequestContext {
        &self.rctx
    }

    /// The Arc reference that provides the context for the query request.
    pub fn request_context_arc(&self) -> Arc<QueryRequestContext> {
        self.rctx.clone()
    }

    /// Obtain the context of the query request (compatible with old code)
    pub fn rctx(&self) -> &QueryRequestContext {
        &self.rctx
    }

    /// The request-scoped cancellation token.
    ///
    /// Threaded into the execution runtime and query registry at instantiation
    /// so all cancellation paths share one source of truth.
    pub fn cancel_token(&self) -> CancelToken {
        self.cancel_token.clone()
    }

    /// Obtaining character set information
    pub fn charset_info(&self) -> Option<&CharsetInfo> {
        self.charset_info.as_ref().map(|ci| ci.as_ref())
    }

    /// Setting character set information
    pub fn set_charset_info(&mut self, charset_info: CharsetInfo) {
        self.charset_info = Some(Box::new(charset_info));
    }

    /// The MVCC snapshot timestamp for consistent reads within explicit
    /// transactions.
    ///
    /// `None` means auto-commit single-statement execution, which reads the
    /// current version of the data.
    pub fn snapshot_ts(&self) -> Option<Timestamp> {
        self.snapshot_ts
    }

    /// The transaction isolation level for this execution, when running
    /// inside an explicit transaction.
    pub fn isolation_level(&self) -> Option<crate::core::types::TransactionIsolationLevel> {
        self.isolation_level
    }

    /// Generate an ID.
    pub fn gen_id(&self) -> i64 {
        self.id_gen.id()
    }

    /// Retrieve the current ID value (without incrementing it).
    pub fn current_id(&self) -> i64 {
        self.id_gen.current_value()
    }

    /// Obtain the current spatial information
    pub fn space_info(&self) -> Option<&SpaceInfo> {
        self.space_info.as_ref()
    }

    /// Set the current space information
    pub fn set_space_info(&mut self, space_info: SpaceInfo) {
        self.space_info = Some(space_info);
    }

    /// Obtain the ID of the current space.
    pub fn space_id(&self) -> Option<u64> {
        self.space_info.as_ref().map(|s| s.space_id)
    }

    /// Get the name of the current space.
    pub fn space_name(&self) -> Option<String> {
        self.space_info.as_ref().map(|s| s.space_name.clone())
    }

    /// Mark this query as killed.
    ///
    /// Cancels the request-scoped token shared with the execution runtime and
    /// the query registry, so running operators abort at the next cooperative
    /// cancellation check and `KILL QUERY` observability reflects the state.
    pub fn mark_killed(&self) {
        self.cancel_token.cancel(CancelReason::UserKill);
    }

    /// Check whether this query has been killed/cancelled.
    pub fn is_killed(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Check whether the parameters exist.
    pub fn exist_parameter(&self, param: &str) -> bool {
        self.rctx.get_parameter(param).is_some()
    }

    /// Obtain the query string
    pub fn query(&self) -> &str {
        &self.rctx.query
    }

    /// Obtain parameters
    pub fn parameters(&self) -> &std::collections::HashMap<String, crate::core::Value> {
        &self.rctx.parameters
    }

    /// Reset the query context
    pub fn reset(&mut self) {
        self.cancel_token.clear();
        self.id_gen.reset(0);
        self.space_info = None;
        self.charset_info = None;
        self.snapshot_ts = None;
        self.isolation_level = None;
        if let Some(ref mut arena) = self.arena {
            arena.reset();
        }
        log::info!("Query context has been reset");
    }

    /// Check if arena allocation is enabled
    pub fn has_arena(&self) -> bool {
        self.arena.is_some()
    }

    /// Get a reference to the arena allocator
    pub fn arena(&self) -> Option<&Arena> {
        self.arena.as_ref()
    }

    /// Get arena memory statistics (allocated_bytes)
    pub fn arena_stats(&self) -> Option<usize> {
        self.arena.as_ref().map(|a| a.allocated_bytes())
    }

    // Note: resource_context() and space_context() methods have been removed
    // as part of the optimization to inline these contexts into QueryContext.
    // Use the direct accessor methods instead:
    // - gen_id(), current_id() for resource operations
    // - space_info(), space_id(), space_name(), charset_info() for space operations
}

impl std::fmt::Debug for QueryContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryContext")
            .field("rctx", &self.rctx)
            .field("space_id", &self.space_id())
            .field("snapshot_ts", &self.snapshot_ts)
            .field("isolation_level", &self.isolation_level)
            .field("killed", &self.is_killed())
            .field("has_arena", &self.arena.is_some())
            .finish()
    }
}

impl Default for QueryContext {
    fn default() -> Self {
        Self::new(Arc::new(QueryRequestContext::default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_killed_cancels_shared_token() {
        let rctx = Arc::new(QueryRequestContext::new("MATCH (n) RETURN n".to_string()));
        let ctx = QueryContext::new(rctx);
        assert!(!ctx.is_killed());

        // A clone of the token is what the pipeline hands to the runtime.
        let runtime_token = ctx.cancel_token();
        assert!(!runtime_token.is_cancelled());

        ctx.mark_killed();

        assert!(ctx.is_killed());
        assert!(runtime_token.is_cancelled());
        assert_eq!(runtime_token.reason(), Some(CancelReason::UserKill));
    }

    #[test]
    fn runtime_adopts_context_token() {
        use crate::executor::streaming::runtime::ExecutionRuntime;
        let rctx = Arc::new(QueryRequestContext::new("MATCH (n) RETURN n".to_string()));
        let ctx = QueryContext::new(rctx);

        let runtime = ExecutionRuntime::default_budget();
        runtime.set_cancel_token(ctx.cancel_token());
        assert!(!runtime.is_cancelled());

        ctx.mark_killed();
        assert!(runtime.is_cancelled());
        assert!(runtime.ensure_not_cancelled().is_err());
    }

    #[test]
    fn reset_clears_cancellation() {
        let rctx = Arc::new(QueryRequestContext::new("MATCH (n) RETURN n".to_string()));
        let mut ctx = QueryContext::new(rctx);
        ctx.mark_killed();
        assert!(ctx.is_killed());

        ctx.reset();
        assert!(!ctx.is_killed());
    }
}
