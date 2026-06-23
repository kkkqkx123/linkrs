use crate::core::error::{QueryError, QueryResult};
use log::{info, warn};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug)]
pub struct QueryContext {
    contexts: Arc<RwLock<HashMap<u32, String>>>,
}

impl Default for QueryContext {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryContext {
    pub fn new() -> Self {
        Self {
            contexts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn add_query(&self, ep_id: u32, query_context: String, session_id: i64) {
        info!("Adding query {} to session {}", ep_id, session_id);
        self.contexts.write().insert(ep_id, query_context);
    }

    pub fn delete_query(&self, ep_id: u32, session_id: i64) {
        info!("Removing query {} from session {}", ep_id, session_id);
        self.contexts.write().remove(&ep_id);
    }

    pub fn find_query(&self, ep_id: u32) -> bool {
        self.contexts.read().contains_key(&ep_id)
    }

    pub fn mark_query_killed(&self, ep_id: u32, session_id: i64) {
        info!(
            "Marking query {} as killed in session {}",
            ep_id, session_id
        );
        self.contexts.write().remove(&ep_id);
    }

    pub fn mark_all_queries_killed(&self, session_id: i64) {
        let query_count = self.active_queries_count();
        info!(
            "Killing all {} queries in session {}",
            query_count, session_id
        );
        self.contexts.write().clear();
    }

    pub fn active_queries_count(&self) -> usize {
        self.contexts.read().len()
    }

    pub fn kill_query(&self, query_id: u32, session_id: i64) -> QueryResult<()> {
        info!(
            "Attempting to kill query {} in session {}",
            query_id, session_id
        );

        if !self.find_query(query_id) {
            warn!("Query {} not found in session {}", query_id, session_id);
            return Err(QueryError::execution(format!(
                "Query not found: {}",
                query_id
            )));
        }

        self.mark_query_killed(query_id, session_id);

        info!(
            "Successfully killed query {} in session {}",
            query_id, session_id
        );
        Ok(())
    }

    pub fn kill_multiple_queries(
        &self,
        query_ids: &[u32],
        session_id: i64,
    ) -> Vec<QueryResult<()>> {
        query_ids
            .iter()
            .map(|&query_id| self.kill_query(query_id, session_id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_context() {
        let context = QueryContext::new();
        assert_eq!(context.active_queries_count(), 0);
        assert!(!context.find_query(1));

        context.add_query(1, "SELECT * FROM user".to_string(), 123);
        assert_eq!(context.active_queries_count(), 1);
        assert!(context.find_query(1));

        context.delete_query(1, 123);
        assert_eq!(context.active_queries_count(), 0);
        assert!(!context.find_query(1));
    }

    #[test]
    fn test_kill_query() {
        let context = QueryContext::new();
        context.add_query(2, "MATCH (n) RETURN n".to_string(), 123);
        let result = context.kill_query(2, 123);
        assert!(result.is_ok());
        assert!(!context.find_query(2));

        let result = context.kill_query(999, 123);
        assert!(result.is_err());
    }

    #[test]
    fn test_kill_multiple_queries() {
        let context = QueryContext::new();
        context.add_query(3, "query 3".to_string(), 123);
        context.add_query(4, "query 4".to_string(), 123);
        let results = context.kill_multiple_queries(&[3, 4, 5], 123);
        assert_eq!(results.len(), 3);
        assert!(results[0].is_ok());
        assert!(results[1].is_ok());
        assert!(results[2].is_err());
    }
}
