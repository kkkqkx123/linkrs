//! Query Request Context – A Simplified Version Dedicated to the Query Layer
//!
//! This module provides the minimum amount of contextual information required to execute the query, thereby avoiding the need for the query layer to rely on the API layer.

use crate::storage::QueryStorage;
use crate::storage::StorageOperationContext;
use graphdb_core::Value;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

/// Query request context – Simplified version
///
/// Contains:
/// - Session ID
/// - User name
/// - Graph Space Name
/// - Query string
/// - Query parameters
/// - Optional caller-assigned query ID (threaded to the runtime)
#[derive(Debug, Clone, Default)]
pub struct QueryRequestContext {
    /// Session ID
    pub session_id: Option<i64>,
    /// Username
    pub user_name: Option<String>,
    /// Name of the graph space
    pub space_name: Option<String>,
    /// Query string
    pub query: String,
    /// Query parameters
    pub parameters: HashMap<String, Value>,
    /// Session variable snapshot (`$name` references), captured once per
    /// statement at the API layer. Distinct from `parameters` (`@name`).
    pub session_variables: HashMap<String, Value>,
    /// Optional caller/server-assigned query ID.
    ///
    /// When set, the execution runtime is registered and identified under
    /// this ID; when absent, the [`QueryRegistry`] allocates a unique id.
    /// [`QueryRegistry`]: crate::executor::streaming::query_registry::QueryRegistry
    pub query_id: Option<u64>,
    /// Transaction identity and storage binding for this execution.
    pub transaction_id: Option<graphdb_core::types::TransactionId>,
    pub auto_commit: bool,
    pub read_only: bool,
    /// Transaction isolation level injected by the API layer for queries
    /// running inside an explicit transaction. `None` = auto-commit
    /// statement-level snapshot semantics.
    pub isolation_level: Option<graphdb_core::types::TransactionIsolationLevel>,
    pub operation_context: Option<StorageOperationContext>,
    pub operation_storage: Option<Arc<RwLock<dyn QueryStorage>>>,
    /// Pre-parsed statement AST supplied by the API layer.
    ///
    /// When present, [`prepare_request`] skips the internal parse of `query`
    /// and uses this AST directly (the classification pass already produced
    /// it). The AST carries its own expression analysis context, so all
    /// expression ids remain consistent with the generated plan.
    ///
    /// [`prepare_request`]: crate::pipeline::prepared::PreparedRequest
    pub parsed_statement: Option<Arc<crate::parser::ast::stmt::Ast>>,
    /// Consistency requirement for secondary-index reads (vector/fulltext).
    /// `None` = eventual; `Some(timeout_ms)` = read-your-writes with timeout.
    pub consistency_timeout_ms: Option<u64>,
    /// Minimum LSN to wait for when `consistency_timeout_ms` is set.
    /// `None` means wait for current materialized LSN.
    pub minimum_lsn: Option<graphdb_core::types::CommitLsn>,
}

impl QueryRequestContext {
    /// Create a new query request context.
    pub fn new(query: String) -> Self {
        Self {
            session_id: None,
            user_name: None,
            space_name: None,
            query,
            parameters: HashMap::new(),
            session_variables: HashMap::new(),
            query_id: None,
            transaction_id: None,
            auto_commit: true,
            read_only: false,
            isolation_level: None,
            operation_context: None,
            operation_storage: None,
            parsed_statement: None,
            consistency_timeout_ms: None,
            minimum_lsn: None,
        }
    }

    /// Create a query request context with parameters
    pub fn with_parameters(mut self, parameters: HashMap<String, Value>) -> Self {
        self.parameters = parameters;
        self
    }

    /// Set the session variable snapshot.
    pub fn with_session_variables(mut self, variables: HashMap<String, Value>) -> Self {
        self.session_variables = variables;
        self
    }

    /// Setting the session ID
    pub fn with_session_id(mut self, session_id: i64) -> Self {
        self.session_id = Some(session_id);
        self
    }

    /// Set the username
    pub fn with_user_name(mut self, user_name: String) -> Self {
        self.user_name = Some(user_name);
        self
    }

    /// Set the name of the graph space.
    pub fn with_space_name(mut self, space_name: String) -> Self {
        self.space_name = Some(space_name);
        self
    }

    /// Set the caller/server-assigned query ID.
    pub fn with_query_id(mut self, query_id: u64) -> Self {
        self.query_id = Some(query_id);
        self
    }

    /// Obtain parameters
    pub fn get_parameter(&self, param: &str) -> Option<Value> {
        self.parameters.get(param).cloned()
    }

    /// Check whether the parameters exist.
    pub fn has_parameter(&self, param: &str) -> bool {
        self.parameters.contains_key(param)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_request_context_new() {
        let ctx = QueryRequestContext::new("MATCH (n) RETURN n".to_string());
        assert_eq!(ctx.query, "MATCH (n) RETURN n");
        assert!(ctx.session_id.is_none());
        assert!(ctx.space_name.is_none());
    }

    #[test]
    fn test_query_request_context_with_params() {
        let mut params = HashMap::new();
        params.insert("name".to_string(), Value::from("test"));

        let ctx = QueryRequestContext::new("QUERY".to_string())
            .with_parameters(params)
            .with_session_id(123)
            .with_space_name("test_space".to_string());

        assert_eq!(ctx.session_id, Some(123));
        assert_eq!(ctx.space_name, Some("test_space".to_string()));
        assert!(ctx.has_parameter("name"));
    }
}
