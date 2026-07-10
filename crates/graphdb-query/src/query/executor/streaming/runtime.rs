use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use crate::core::error::QueryError;
use crate::query::executor::base::MemoryBudget;

/// Query identity information
#[derive(Debug, Clone, Default)]
pub struct QueryIdentity {
    pub query_id: u64,
    pub session_id: Option<String>,
    pub space_name: Option<String>,
}

/// Per-operator profile snapshot
#[derive(Debug, Clone, Default)]
pub struct OperatorProfile {
    pub node_id: i64,
    pub name: String,
    pub open_time_us: u64,
    pub next_time_us: u64,
    pub close_time_us: u64,
    pub input_rows: u64,
    pub output_rows: u64,
    pub peak_memory: u64,
}

/// Collects execution profile data across all operators
#[derive(Debug, Default)]
pub struct ProfileCollector {
    pub operators: HashMap<i64, OperatorProfile>,
    pub total_rows: u64,
    pub total_time_us: u64,
    pub start_time: Option<Instant>,
    pub end_time: Option<Instant>,
}

impl ProfileCollector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_start(&mut self) {
        self.start_time = Some(Instant::now());
    }

    pub fn record_end(&mut self) {
        self.end_time = Some(Instant::now());
        if let Some(start) = self.start_time {
            self.total_time_us = start.elapsed().as_micros() as u64;
        }
    }

    pub fn add_rows(&mut self, count: u64) {
        self.total_rows += count;
    }

    pub fn record_operator_profile(&mut self, profile: OperatorProfile) {
        self.operators.insert(profile.node_id, profile);
    }
}

/// Manages cleanup of runtime resources (cursors, temp files, etc.)
pub struct ResourceOwner {
    cleanup: Vec<Box<dyn FnOnce() + Send>>,
}

impl std::fmt::Debug for ResourceOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceOwner")
            .field("cleanup_count", &self.cleanup.len())
            .finish()
    }
}

impl Default for ResourceOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceOwner {
    pub fn new() -> Self {
        Self { cleanup: Vec::new() }
    }

    pub fn add(&mut self, cleanup: Box<dyn FnOnce() + Send>) {
        self.cleanup.push(cleanup);
    }

    pub fn release_all(&mut self) {
        for f in self.cleanup.drain(..) {
            f();
        }
    }
}

/// Per-query execution runtime shared across all operators.
///
/// Centralises cancellation, memory tracking, profiling, and resource
/// lifecycle so that operators do not each carry ad-hoc context.
///
/// Phase 1: engine-level cancel checking and basic profile tracking.
/// Future phases add per-operator cancel checking, spill, and full
/// instrumentation.
#[derive(Debug)]
pub struct ExecutionRuntime {
    /// Query identity
    query_id: QueryIdentity,
    /// Set to `true` when the query should be cancelled.
    cancel_token: Arc<AtomicBool>,
    /// Optional deadline; the query is cancelled after this instant.
    deadline: Option<Instant>,
    /// Per-query memory budget for blocking operators.
    pub memory_budget: MemoryBudget,
    /// Profile collector (behind a mutex so operators can record stats).
    profile: Arc<Mutex<ProfileCollector>>,
    /// Resource owner for cleanup of cursors, temp files, etc.
    resource_owner: Arc<Mutex<ResourceOwner>>,
}

impl ExecutionRuntime {
    /// Create a new execution runtime with the given query identity and memory budget.
    pub fn new(query_id: QueryIdentity, memory_budget: MemoryBudget) -> Self {
        Self {
            query_id,
            cancel_token: Arc::new(AtomicBool::new(false)),
            deadline: None,
            memory_budget,
            profile: Arc::new(Mutex::new(ProfileCollector::new())),
            resource_owner: Arc::new(Mutex::new(ResourceOwner::new())),
        }
    }

    /// Create a runtime with default settings (query_id = 0, default memory budget).
    pub fn default_budget() -> Self {
        Self::new(QueryIdentity::default(), MemoryBudget::default_budget())
    }

    // ── Query identity ──

    pub fn query_id(&self) -> &QueryIdentity {
        &self.query_id
    }

    // ── Cancellation ──

    /// Token used to signal cancellation (shared with operators and I/O).
    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancel_token.clone()
    }

    /// Check whether the query has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.load(Ordering::Relaxed)
            || self.deadline.map_or(false, |d| Instant::now() >= d)
    }

    /// Return an error if the query has been cancelled.
    pub fn ensure_not_cancelled(&self) -> Result<(), QueryError> {
        if self.is_cancelled() {
            Err(QueryError::execution("Query cancelled".to_string()))
        } else {
            Ok(())
        }
    }

    /// Cancel this query (set the cancel token).
    pub fn cancel(&self) {
        self.cancel_token.store(true, Ordering::Relaxed);
    }

    /// Set or clear a deadline.
    pub fn set_deadline(&mut self, deadline: Option<Instant>) {
        self.deadline = deadline;
    }

    // ── Profile ──

    pub fn profile(&self) -> &Arc<Mutex<ProfileCollector>> {
        &self.profile
    }

    /// Record that execution has started (profile timing).
    pub fn profile_start(&self) {
        self.profile.lock().record_start();
    }

    /// Record that execution has ended.
    pub fn profile_end(&self) {
        self.profile.lock().record_end();
    }

    /// Add rows to the profile counter.
    pub fn profile_add_rows(&self, count: u64) {
        self.profile.lock().add_rows(count);
    }

    // ── Resource ownership ──

    pub fn resource_owner(&self) -> &Arc<Mutex<ResourceOwner>> {
        &self.resource_owner
    }

    /// Register a cleanup callback.
    pub fn on_cleanup<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.resource_owner.lock().add(Box::new(f));
    }

    /// Release all owned resources.
    pub fn release_resources(&self) {
        self.resource_owner.lock().release_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_default_budget() {
        let rt = ExecutionRuntime::default_budget();
        assert!(!rt.is_cancelled());
        assert_eq!(rt.query_id().query_id, 0);
    }

    #[test]
    fn test_cancel_token() {
        let rt = ExecutionRuntime::default_budget();
        assert!(!rt.is_cancelled());
        rt.cancel();
        assert!(rt.is_cancelled());
        assert!(rt.ensure_not_cancelled().is_err());
    }

    #[test]
    fn test_deadline() {
        let mut rt = ExecutionRuntime::default_budget();
        rt.set_deadline(Some(Instant::now()));
        assert!(rt.is_cancelled());
    }

    #[test]
    fn test_profile_collector() {
        let mut pc = ProfileCollector::new();
        pc.record_start();
        std::thread::sleep(std::time::Duration::from_micros(100));
        pc.record_end();
        assert!(pc.total_time_us > 0);
    }

    #[test]
    fn test_profile_add_rows() {
        let rt = ExecutionRuntime::default_budget();
        rt.profile_add_rows(10);
        rt.profile_add_rows(20);
        assert_eq!(rt.profile().lock().total_rows, 30);
    }

    #[test]
    fn test_resource_owner() {
        let mut owner = ResourceOwner::new();
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = flag.clone();
        owner.add(Box::new(move || {
            f.store(true, std::sync::atomic::Ordering::Relaxed);
        }));
        owner.release_all();
        assert!(flag.load(std::sync::atomic::Ordering::Relaxed));
    }
}
