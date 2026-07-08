//! Query Request Context – A Simplified Version Dedicated to the Query Layer
//!
//! This module provides the minimum amount of contextual information required to execute the query, thereby avoiding the need for the query layer to rely on the API layer.

use crate::core::Value;
use std::collections::HashMap;

/// Query request context – Simplified version
///
/// Contains:
/// - Session ID
/// - User name
/// - Graph Space Name
/// - Query string
/// - Query parameters
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
        }
    }

    /// Create a query request context with parameters
    pub fn with_parameters(mut self, parameters: HashMap<String, Value>) -> Self {
        self.parameters = parameters;
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
