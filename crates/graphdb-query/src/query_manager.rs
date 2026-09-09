//! Query Manager
//!
//! Responsible for tracking and managing the queries that are currently in progress.

use dashmap::DashMap;
use log::{info, warn};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use graphdb_core::error::{ManagerError, ManagerResult};

use super::session_events::{SessionEvent, SessionEventCallback};
use graphdb_core::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};

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

/// Query Manager
pub struct QueryManager {
    queries: DashMap<i64, QueryInfo>,
    next_query_id: AtomicI64,
    session_callbacks: EventSubscriptions<SessionEvent>,
    slow_query_threshold_ms: AtomicI64,
}

impl std::fmt::Debug for QueryManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryManager")
            .field("queries_count", &self.queries.len())
            .field("session_callbacks", &self.session_callbacks.len())
            .field(
                "slow_query_threshold_ms",
                &self.slow_query_threshold_ms.load(Ordering::SeqCst),
            )
            .finish()
    }
}

impl QueryManager {
    pub fn new() -> Self {
        Self {
            queries: DashMap::new(),
            next_query_id: AtomicI64::new(1),
            session_callbacks: EventSubscriptions::new(),
            slow_query_threshold_ms: AtomicI64::new(1000),
        }
    }

    /// Register a runtime observer for session/query events.
    pub fn register_session_callback(&self, callback: SessionEventCallback) -> SubscriptionId {
        self.session_callbacks.add(callback)
    }

    /// Register a filtered observer invoked only when `filter` returns true.
    pub fn register_session_callback_filtered(
        &self,
        callback: SessionEventCallback,
        filter: EventFilter<SessionEvent>,
    ) -> SubscriptionId {
        self.session_callbacks.add_filtered(callback, Some(filter))
    }

    /// Remove a previously registered observer. Returns true if present.
    pub fn unregister_session_callback(&self, id: SubscriptionId) -> bool {
        self.session_callbacks.remove(id)
    }

    /// Number of registered session observers.
    pub fn session_callback_count(&self) -> usize {
        self.session_callbacks.len()
    }

    /// Configure the threshold used to emit `SlowQueryDetected`.
    pub fn set_slow_query_threshold_ms(&self, threshold_ms: i64) {
        self.slow_query_threshold_ms
            .store(threshold_ms, Ordering::SeqCst);
    }

    /// Current slow-query threshold in milliseconds.
    pub fn slow_query_threshold_ms(&self) -> i64 {
        self.slow_query_threshold_ms.load(Ordering::SeqCst)
    }

    fn emit_session_event(&self, event: SessionEvent) {
        if self.session_callbacks.is_empty() {
            // Slow queries are still worth a log line even without observers.
            if let SessionEvent::SlowQueryDetected {
                session_id,
                query_id,
                duration_ms,
                threshold_ms,
            } = &event
            {
                warn!(
                    "Slow query detected: session={}, query={}, duration={}ms threshold={}ms",
                    session_id, query_id, duration_ms, threshold_ms
                );
            }
            return;
        }
        self.session_callbacks.dispatch("session", &event);
    }

    fn check_slow_query(&self, session_id: i64, query_id: i64, duration_ms: i64) {
        let threshold = self.slow_query_threshold_ms.load(Ordering::SeqCst);
        if duration_ms >= threshold {
            self.emit_session_event(SessionEvent::SlowQueryDetected {
                session_id,
                query_id,
                duration_ms,
                threshold_ms: threshold,
            });
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

        self.emit_session_event(SessionEvent::QueryStarted {
            session_id,
            query_id,
            query_text,
        });

        query_id
    }

    /// Complete the query.
    pub fn finish_query(&self, query_id: i64) -> ManagerResult<()> {
        if let Some(mut query) = self.queries.get_mut(&query_id) {
            query.finish();
            let duration_ms = query.duration_ms.unwrap_or(0);
            let session_id = query.session_id;
            info!(
                "Query finished: id={}, duration={}ms",
                query_id, duration_ms
            );
            drop(query);
            self.emit_session_event(SessionEvent::QueryCompleted {
                session_id,
                query_id,
                duration_ms,
                success: true,
            });
            self.check_slow_query(session_id, query_id, duration_ms);
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
            let duration_ms = query.duration_ms.unwrap_or(0);
            let session_id = query.session_id;
            warn!("Query failed: id={}, duration={}ms", query_id, duration_ms);
            drop(query);
            self.emit_session_event(SessionEvent::QueryCompleted {
                session_id,
                query_id,
                duration_ms,
                success: false,
            });
            self.check_slow_query(session_id, query_id, duration_ms);
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
            let duration_ms = query.duration_ms.unwrap_or(0);
            let session_id = query.session_id;
            warn!("Query killed: id={}", query_id);
            drop(query);
            self.emit_session_event(SessionEvent::QueryCompleted {
                session_id,
                query_id,
                duration_ms,
                success: false,
            });
            self.check_slow_query(session_id, query_id, duration_ms);
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
