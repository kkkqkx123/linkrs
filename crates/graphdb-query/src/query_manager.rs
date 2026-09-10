//! Query Manager
//!
//! Responsible for tracking and managing the queries that are currently in progress.

use dashmap::DashMap;
use log::{info, warn};
use parking_lot::RwLock;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use graphdb_core::error::{ManagerError, ManagerResult};

use super::session_events::{SessionEvent, SessionEventCallback};
use crate::executor::streaming::query_registry::{QueryId, QueryRegistry};
use crate::executor::streaming::transaction_scope::CancelReason;
use graphdb_core::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};

/// Row-count progress notification for a running query.
///
/// Independent lightweight hook: deliberately not a `SessionEvent`
/// variant, so high-frequency progress never touches the low-frequency
/// session-lifecycle matches. Zero overhead when no observer is registered.
/// The executor calls `emit_progress` only at its configured row interval.
/// The notification is valid only for the dispatch call; observers must
/// not retain it beyond the callback.
#[derive(Debug, Clone)]
pub struct QueryProgress {
    pub session_id: i64,
    pub query_id: i64,
    pub rows_processed: u64,
}

/// Runtime observer for query progress.
pub type QueryProgressCallback = Arc<dyn Fn(&QueryProgress) + Send + Sync>;

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
///
/// Single enum dual emission source: query lifecycle halves
/// (`QueryStarted` / `QueryCompleted` / `SlowQueryDetected`) are emitted here,
/// session lifecycle halves (`SessionCreated` / `SessionDestroyed`) by
/// `GraphSessionManager`. Both sides can share one
/// `Arc<EventSubscriptions<SessionEvent>>` via `new_with_shared` so observers
/// subscribe once and receive both halves.
pub struct QueryManager {
    queries: DashMap<i64, QueryInfo>,
    next_query_id: AtomicI64,
    session_callbacks: Arc<EventSubscriptions<SessionEvent>>,
    progress_callbacks: Arc<EventSubscriptions<QueryProgress>>,
    // Reverse bridge for KILL QUERY: when the assembly registers the
    // executor's QueryRegistry here, kill_query also cancels the token.
    // Absent mapping degrades to status-marking only.
    query_registry: RwLock<Option<Arc<QueryRegistry>>>,
    slow_query_threshold_ms: AtomicI64,
}

impl std::fmt::Debug for QueryManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QueryManager")
            .field("queries_count", &self.queries.len())
            .field("session_callbacks", &self.session_callbacks.len())
            .field("progress_callbacks", &self.progress_callbacks.len())
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
            session_callbacks: Arc::new(EventSubscriptions::new()),
            progress_callbacks: Arc::new(EventSubscriptions::new()),
            query_registry: RwLock::new(None),
            slow_query_threshold_ms: AtomicI64::new(1000),
        }
    }

    /// Build a manager sharing one session-event registry with
    /// `GraphSessionManager`.
    pub fn new_with_shared(shared: Arc<EventSubscriptions<SessionEvent>>) -> Self {
        Self {
            queries: DashMap::new(),
            next_query_id: AtomicI64::new(1),
            session_callbacks: shared,
            progress_callbacks: Arc::new(EventSubscriptions::new()),
            query_registry: RwLock::new(None),
            slow_query_threshold_ms: AtomicI64::new(1000),
        }
    }

    /// Shared session-event registry behind this manager.
    pub fn shared_session_callbacks(&self) -> Arc<EventSubscriptions<SessionEvent>> {
        Arc::clone(&self.session_callbacks)
    }

    /// Point this manager at a shared session-event registry.
    pub fn set_shared_session_callbacks(&mut self, shared: Arc<EventSubscriptions<SessionEvent>>) {
        self.session_callbacks = shared;
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

    /// Attach the executor's query registry so `kill_query` also cancels the
    /// execution token. Without it, `kill_query` only marks the query status.
    pub fn set_query_registry(&self, registry: Arc<QueryRegistry>) {
        *self.query_registry.write() = Some(registry);
    }

    /// Detach the executor's query registry; `kill_query` returns to
    /// status-marking only.
    pub fn clear_query_registry(&self) {
        *self.query_registry.write() = None;
    }

    /// Register a progress observer. The executor invokes it at its own row
    /// interval; unregistered by default, hence zero hot-path overhead.
    pub fn register_progress_callback(&self, callback: QueryProgressCallback) -> SubscriptionId {
        self.progress_callbacks.add(callback)
    }

    /// Register a progress observer thinned to every `rows_interval` rows.
    ///
    /// Observer-side thinning: deliveries whose `rows_processed` is not a
    /// multiple of `rows_interval` are dropped before reaching `callback`.
    /// `rows_interval == 0` disables thinning (every emission is delivered).
    /// The executor still decides when to call `emit_progress`; pairing a
    /// coarse executor cadence with a fine observer interval costs nothing
    /// extra, while a fine executor cadence with a coarse observer interval
    /// only pays for the dropped filter checks.
    pub fn register_progress_callback_with_interval(
        &self,
        callback: QueryProgressCallback,
        rows_interval: u64,
    ) -> SubscriptionId {
        if rows_interval == 0 {
            return self.progress_callbacks.add(callback);
        }
        self.progress_callbacks.add_filtered(
            callback,
            Some(Arc::new(move |progress: &QueryProgress| {
                progress.rows_processed.is_multiple_of(rows_interval)
            })),
        )
    }

    /// Register a filtered progress observer.
    pub fn register_progress_callback_filtered(
        &self,
        callback: QueryProgressCallback,
        filter: EventFilter<QueryProgress>,
    ) -> SubscriptionId {
        self.progress_callbacks.add_filtered(callback, Some(filter))
    }

    /// Remove a previously registered progress observer.
    pub fn unregister_progress_callback(&self, id: SubscriptionId) -> bool {
        self.progress_callbacks.remove(id)
    }

    /// Number of registered progress observers.
    pub fn progress_callback_count(&self) -> usize {
        self.progress_callbacks.len()
    }

    /// Emit a progress notification. No-op when nobody listens.
    ///
    /// Explicit executor entry point: streaming operators call this when
    /// their processed row count reaches the configured cadence. Callers
    /// outside the executor (tests, manual drivers) may invoke it directly.
    pub fn emit_progress(&self, session_id: i64, query_id: i64, rows_processed: u64) {
        if self.progress_callbacks.is_empty() {
            return;
        }
        self.progress_callbacks.dispatch(
            "progress",
            &QueryProgress {
                session_id,
                query_id,
                rows_processed,
            },
        );
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
    ///
    /// Marks the query Killed and, when a query registry is attached via
    /// `set_query_registry`, cancels its execution token with
    /// `CancelReason::UserKill` so a running query actually stops.
    /// Without an attached registry this only marks the status; the
    /// executor-side `QueryRegistry` remains the source of truth for
    /// cancellation. Query ids are generated as positive `i64` values, so a
    /// negative id is rejected as invalid input instead of wrapping on the
    /// `i64` to executor `u64` id conversion.
    pub fn kill_query(&self, query_id: i64) -> ManagerResult<()> {
        if query_id < 0 {
            return Err(ManagerError::InvalidInput(format!(
                "invalid query id {query_id}"
            )));
        }
        if let Some(mut query) = self.queries.get_mut(&query_id) {
            query.kill();
            let duration_ms = query.duration_ms.unwrap_or(0);
            let session_id = query.session_id;
            warn!("Query killed: id={}", query_id);
            drop(query);
            if let Some(registry) = self.query_registry.read().as_ref() {
                registry.cancel(QueryId(query_id as u64), CancelReason::UserKill);
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::streaming::query_registry::QueryMetadata;
    use std::sync::atomic::AtomicUsize;
    use std::time::Instant;

    #[test]
    fn kill_query_cancels_registry_token_when_attached() {
        let manager = QueryManager::new();
        let registry = Arc::new(QueryRegistry::new());
        manager.set_query_registry(Arc::clone(&registry));

        let query_id = manager.register_query(
            7,
            "tester".to_string(),
            None,
            "MATCH (n) RETURN n".to_string(),
        );
        // Mirror the executor assembly: registry entry under the same id.
        let (registry_id, token) = registry.register_with_id(
            QueryId(query_id as u64),
            QueryMetadata {
                query_id: QueryId(query_id as u64),
                session_id: Some(7),
                user_name: Some("tester".to_string()),
                space_name: None,
                query_text: Some("MATCH (n) RETURN n".to_string()),
                start_time: Instant::now(),
            },
        );
        assert_eq!(registry_id, QueryId(query_id as u64));

        manager.kill_query(query_id).expect("kill must succeed");
        assert!(token.is_cancelled());
        assert_eq!(
            token.reason(),
            Some(CancelReason::UserKill),
            "kill must propagate the typed reason"
        );
        assert_eq!(
            manager.get_query(query_id).map(|q| q.status),
            Some(QueryStatus::Killed)
        );
    }

    #[test]
    fn kill_query_without_registry_only_marks_status() {
        let manager = QueryManager::new();
        let query_id = manager.register_query(
            7,
            "tester".to_string(),
            None,
            "MATCH (n) RETURN n".to_string(),
        );
        manager.kill_query(query_id).expect("kill must succeed");
        assert_eq!(
            manager.get_query(query_id).map(|q| q.status),
            Some(QueryStatus::Killed)
        );
    }

    #[test]
    fn progress_hook_receives_emissions_and_unsubscribes() {
        let manager = QueryManager::new();
        assert_eq!(manager.progress_callback_count(), 0);
        // No listeners: no-op, must not panic.
        manager.emit_progress(1, 1, 100);

        let rows = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&rows);
        let id = manager.register_progress_callback(Arc::new(move |progress| {
            assert_eq!(progress.session_id, 1);
            assert_eq!(progress.query_id, 2);
            probe.fetch_add(progress.rows_processed as usize, Ordering::SeqCst);
        }));
        manager.emit_progress(1, 2, 100);
        manager.emit_progress(1, 2, 50);
        assert_eq!(rows.load(Ordering::SeqCst), 150);
        assert!(manager.unregister_progress_callback(id));
        manager.emit_progress(1, 2, 100);
        assert_eq!(rows.load(Ordering::SeqCst), 150);
    }
}
