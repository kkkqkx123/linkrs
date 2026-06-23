//! Query Manager
//!
//! Responsible for tracking and managing the queries that are currently in progress.

use dashmap::DashMap;
use log::{info, warn};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::error::{ManagerError, ManagerResult};

/// Query status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryStatus {
    Running,
    Finished,
    Failed,
    Killed,
}

/// Query information
#[derive(Debug, Clone)]
pub struct QueryInfo {
    pub query_id: i64,
    pub session_id: i64,
    pub user_name: String,
    pub space_name: Option<String>,
    pub query_text: String,
    pub status: QueryStatus,
    pub start_time: SystemTime,
    pub duration_ms: Option<i64>,
    pub execution_plan: Option<String>,
}

impl QueryInfo {
    pub fn new(
        query_id: i64,
        session_id: i64,
        user_name: String,
        space_name: Option<String>,
        query_text: String,
    ) -> Self {
        Self {
            query_id,
            session_id,
            user_name,
            space_name,
            query_text,
            status: QueryStatus::Running,
            start_time: SystemTime::now(),
            duration_ms: None,
            execution_plan: None,
        }
    }

    pub fn finish(&mut self) {
        self.status = QueryStatus::Finished;
        self.duration_ms = Some(
            SystemTime::now()
                .duration_since(self.start_time)
                .unwrap_or_default()
                .as_millis() as i64,
        );
    }

    pub fn fail(&mut self) {
        self.status = QueryStatus::Failed;
        self.duration_ms = Some(
            SystemTime::now()
                .duration_since(self.start_time)
                .unwrap_or_default()
                .as_millis() as i64,
        );
    }

    pub fn kill(&mut self) {
        self.status = QueryStatus::Killed;
        self.duration_ms = Some(
            SystemTime::now()
                .duration_since(self.start_time)
                .unwrap_or_default()
                .as_millis() as i64,
        );
    }
}

/// Legacy query statistics (for backward compatibility)
#[derive(Debug, Clone, Default)]
pub struct QueryStats {
    pub total_queries: u64,
    pub running_queries: u64,
    pub finished_queries: u64,
    pub failed_queries: u64,
    pub killed_queries: u64,
    pub avg_duration_ms: i64,
}

/// Query Manager
pub struct QueryManager {
    queries: DashMap<i64, QueryInfo>,
    next_query_id: AtomicI64,
}

impl QueryManager {
    pub fn new() -> Self {
        Self {
            queries: DashMap::new(),
            next_query_id: AtomicI64::new(1),
        }
    }

    /// Generate a new query ID.
    fn generate_query_id(&self) -> i64 {
        self.next_query_id.fetch_add(1, Ordering::SeqCst)
    }

    /// Register a new query
    pub fn register_query(
        &self,
        session_id: i64,
        user_name: String,
        space_name: Option<String>,
        query_text: String,
    ) -> i64 {
        let query_id = self.generate_query_id();
        let query_info = QueryInfo::new(
            query_id,
            session_id,
            user_name,
            space_name,
            query_text.clone(),
        );

        self.queries.insert(query_id, query_info);

        info!(
            "Query registered: id={}, session_id={}, query={}",
            query_id, session_id, query_text
        );

        query_id
    }

    /// Complete the query.
    pub fn finish_query(&self, query_id: i64) -> ManagerResult<()> {
        if let Some(mut query) = self.queries.get_mut(&query_id) {
            query.finish();
            info!(
                "Query finished: id={}, duration={}ms",
                query_id,
                query.duration_ms.unwrap_or(0)
            );
            Ok(())
        } else {
            Err(ManagerError::NotFound(format!(
                "Query {} not found",
                query_id
            )))
        }
    }

    /// The marker query failed.
    pub fn fail_query(&self, query_id: i64) -> ManagerResult<()> {
        if let Some(mut query) = self.queries.get_mut(&query_id) {
            query.fail();
            warn!(
                "Query failed: id={}, duration={}ms",
                query_id,
                query.duration_ms.unwrap_or(0)
            );
            Ok(())
        } else {
            Err(ManagerError::NotFound(format!(
                "Query {} not found",
                query_id
            )))
        }
    }

    /// Terminate the query.
    pub fn kill_query(&self, query_id: i64) -> ManagerResult<()> {
        if let Some(mut query) = self.queries.get_mut(&query_id) {
            query.kill();
            warn!("Query killed: id={}", query_id);
            Ok(())
        } else {
            Err(ManagerError::NotFound(format!(
                "Query {} not found",
                query_id
            )))
        }
    }

    /// Obtain the query information
    pub fn get_query(&self, query_id: i64) -> Option<QueryInfo> {
        self.queries.get(&query_id).map(|v| v.clone())
    }

    /// Retrieve all queries
    pub fn get_all_queries(&self) -> Vec<QueryInfo> {
        self.queries.iter().map(|v| v.value().clone()).collect()
    }

    /// Obtain the queries that are currently running.
    pub fn get_running_queries(&self) -> Vec<QueryInfo> {
        self.queries
            .iter()
            .filter(|q| q.value().status == QueryStatus::Running)
            .map(|v| v.value().clone())
            .collect()
    }

    /// Obtain query statistics
    pub fn get_stats(&self) -> QueryStats {
        let queries: Vec<_> = self.queries.iter().map(|v| v.value().clone()).collect();
        let total = queries.len() as u64;
        let running = queries
            .iter()
            .filter(|q| q.status == QueryStatus::Running)
            .count() as u64;
        let finished = queries
            .iter()
            .filter(|q| q.status == QueryStatus::Finished)
            .count() as u64;
        let failed = queries
            .iter()
            .filter(|q| q.status == QueryStatus::Failed)
            .count() as u64;
        let killed = queries
            .iter()
            .filter(|q| q.status == QueryStatus::Killed)
            .count() as u64;

        let total_duration: i64 = queries.iter().filter_map(|q| q.duration_ms).sum();

        let avg_duration = if total > 0 {
            total_duration / total as i64
        } else {
            0
        };

        QueryStats {
            total_queries: total,
            running_queries: running,
            finished_queries: finished,
            failed_queries: failed,
            killed_queries: killed,
            avg_duration_ms: avg_duration,
        }
    }

    /// Retrieve query statistics (returns data of the Result type, compatible with older code)
    pub fn get_query_stats(&self) -> ManagerResult<QueryStats> {
        Ok(self.get_stats())
    }

    /// Clean up the completed queries (retaining only the last N of them).
    pub fn cleanup_finished_queries(&self, keep_count: usize) {
        let mut finished_queries: Vec<_> = self
            .queries
            .iter()
            .filter(|q| q.value().status != QueryStatus::Running)
            .map(|q| *q.key())
            .collect();

        // Sort by start time, keeping the most recent items at the top.
        finished_queries.sort_by_key(|id| {
            self.queries
                .get(id)
                .map(|q| q.start_time)
                .unwrap_or(UNIX_EPOCH)
        });

        let to_remove = finished_queries.len().saturating_sub(keep_count);
        for id in finished_queries.into_iter().take(to_remove) {
            self.queries.remove(&id);
        }
    }
}

impl Default for QueryManager {
    fn default() -> Self {
        Self::new()
    }
}
