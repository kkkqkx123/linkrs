use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use crate::core::error::QueryError;
use crate::query::executor::base::MemoryBudget;
use crate::query::executor::streaming::pool::MorselWorkerPool;
use crate::query::query_manager::QueryManager;

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
    pub partition_id: Option<usize>,
    pub name: String,
    pub open_time_us: u64,
    pub next_time_us: u64,
    pub close_time_us: u64,
    pub output_rows: u64,
    pub peak_memory: u64,
    pub peak_memory_bytes: u64,
    pub spilled_bytes: u64,
    pub spill_count: u64,
}

/// Identifies an operator instance in a partitioned executor tree.
///
/// A plan node occurs once per local partition, so `node_id` alone is not a
/// unique profile key. Global and gather operators use `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OperatorProfileKey {
    pub node_id: i64,
    pub partition_id: Option<usize>,
}

impl OperatorProfileKey {
    pub const fn new(node_id: i64, partition_id: Option<usize>) -> Self {
        Self {
            node_id,
            partition_id,
        }
    }
}

/// Collects execution profile data across all operators
#[derive(Debug, Default)]
pub struct ProfileCollector {
    pub operators: HashMap<OperatorProfileKey, OperatorProfile>,
    pub total_rows: u64,
    pub total_time_us: u64,
    pub start_time: Option<Instant>,
    pub end_time: Option<Instant>,
    /// Wall-clock time spent in parallel partition execution (P8).
    pub parallel_wall_time_us: u64,
    /// Sum of per-worker execution time (may exceed wall time, P8).
    pub parallel_work_time_us: u64,
    /// Maximum number of P8 workers used by any coordinator in this query.
    pub parallel_workers: usize,
    /// Peak number of chunks retained in P8 output queues.
    pub parallel_buffered_chunks_peak: usize,
    /// Peak accounted bytes retained in P8 output queues.
    pub parallel_buffered_bytes_peak: usize,
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
        let key = OperatorProfileKey::new(profile.node_id, profile.partition_id);
        self.operators.insert(key, profile);
    }

    /// Return a snapshot of the P8 parallel profile fields for
    /// EXPLAIN / PROFILE output.
    pub fn parallel_profile(&self) -> (u64, u64, usize, usize, usize) {
        (
            self.parallel_wall_time_us,
            self.parallel_work_time_us,
            self.parallel_workers,
            self.parallel_buffered_chunks_peak,
            self.parallel_buffered_bytes_peak,
        )
    }

    /// Aggregate profiles from partition execution into this collector.
    ///
    /// For each operator node_id, sums timing/output_rows and takes the max
    /// of peak_memory_bytes across partitions.
    pub fn aggregate_partition_profiles(&mut self, partition_profiles: &[ProfileCollector]) {
        for pp in partition_profiles {
            for (key, op) in &pp.operators {
                let entry = self
                    .operators
                    .entry(*key)
                    .or_insert_with(|| OperatorProfile {
                        node_id: key.node_id,
                        partition_id: key.partition_id,
                        name: op.name.clone(),
                        ..OperatorProfile::default()
                    });
                entry.open_time_us += op.open_time_us;
                entry.next_time_us += op.next_time_us;
                entry.close_time_us += op.close_time_us;
                entry.output_rows += op.output_rows;
                entry.peak_memory_bytes = entry.peak_memory_bytes.max(op.peak_memory_bytes);
                entry.spill_count += op.spill_count;
                entry.spilled_bytes += op.spilled_bytes;
            }
            self.total_rows += pp.total_rows;
        }
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
        Self {
            cleanup: Vec::new(),
        }
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
/// Centralises cancellation, memory tracking, profiling, resource
/// lifecycle, and query-registration so that operators do not each
/// carry ad-hoc context.
///
/// Phase 1: engine-level cancel checking and basic profile tracking.
/// Future phases add per-operator cancel checking, spill, and full
/// instrumentation.
#[derive(Debug)]
pub struct ExecutionRuntime {
    /// Query identity (behind Mutex for write-once from the API layer).
    query_id: parking_lot::Mutex<QueryIdentity>,
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
    /// Optional reference to the global QueryManager for KILL QUERY.
    query_manager: Option<Arc<QueryManager>>,
    /// Query-level morsel worker pool for dynamic partition execution.
    /// Created when `max_workers > 1`; `None` means serial fallback.
    pub worker_pool: Option<Arc<MorselWorkerPool>>,
}

impl ExecutionRuntime {
    /// Create a new execution runtime with the given query identity and memory budget.
    pub fn new(query_id: QueryIdentity, memory_budget: MemoryBudget) -> Self {
        Self {
            query_id: parking_lot::Mutex::new(query_id),
            cancel_token: Arc::new(AtomicBool::new(false)),
            deadline: None,
            memory_budget,
            profile: Arc::new(Mutex::new(ProfileCollector::new())),
            resource_owner: Arc::new(Mutex::new(ResourceOwner::new())),
            query_manager: None,
            worker_pool: None,
        }
    }

    /// Create a runtime with default settings (query_id = 0, default memory budget).
    pub fn default_budget() -> Self {
        Self::new(QueryIdentity::default(), MemoryBudget::default_budget())
    }

    // ── Query identity ──

    pub fn query_id(&self) -> QueryIdentity {
        self.query_id.lock().clone()
    }

    /// Override the query ID number after construction.
    ///
    /// The factory initialises `query_id.query_id` to 0; the API layer assigns the
    /// real server-side ID before the handle is returned to the caller.
    pub fn assign_query_id(&self, id: u64) {
        self.query_id.lock().query_id = id;
    }

    /// Attach a QueryManager so that KILL QUERY and finish tracking work.
    pub fn set_query_manager(&mut self, qm: Arc<QueryManager>) {
        self.query_manager = Some(qm);
    }

    /// Register this query with the attached QueryManager and return a
    /// [`QueryFinishGuard`] that marks it finished on drop.
    ///
    /// Returns `None` when no QueryManager is attached (non-fatal).
    pub fn finish_guard(&self) -> Option<QueryFinishGuard> {
        let qm = self.query_manager.as_ref()?.clone();
        let id = self.query_id();
        Some(QueryFinishGuard::new(qm, id.query_id as i64))
    }

    // ── Cancellation ──

    /// Token used to signal cancellation (shared with operators and I/O).
    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        self.cancel_token.clone()
    }

    /// Check whether the query has been cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.load(Ordering::Relaxed)
            || self.deadline.is_some_and(|d| Instant::now() >= d)
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
    ///
    /// Also marks the query as Killed in the attached QueryManager.
    pub fn cancel(&self) {
        self.cancel_token.store(true, Ordering::Relaxed);
        if let Some(ref qm) = self.query_manager {
            let id = self.query_id();
            let _ = qm.kill_query(id.query_id as i64);
        }
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

    /// Set the morsel worker pool for this query. When configured, Exchange
    /// operators use the pool's bounded workers for parallel partition execution
    /// with dynamic morsel-style task assignment.
    pub fn set_worker_pool(&mut self, pool: Option<MorselWorkerPool>) {
        self.worker_pool = pool.map(Arc::new);
    }
}

/// RAII guard that marks a query as finished in the QueryManager on drop.
///
/// Created by [`ExecutionRuntime::finish_guard`]. Ensures the query
/// lifecycle is tracked even when the caller forgets to call finish
/// explicitly, or when execution panics mid-flight.
#[derive(Debug)]
pub struct QueryFinishGuard {
    query_manager: Arc<QueryManager>,
    query_id: i64,
    finished: bool,
}

impl QueryFinishGuard {
    pub fn new(query_manager: Arc<QueryManager>, query_id: i64) -> Self {
        Self {
            query_manager,
            query_id,
            finished: false,
        }
    }

    /// Mark the query as finished immediately without waiting for Drop.
    pub fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            let _ = self.query_manager.finish_query(self.query_id);
        }
    }
}

impl Drop for QueryFinishGuard {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.query_manager.finish_query(self.query_id);
        }
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
    fn test_partition_profile_aggregation_preserves_partition_identity() {
        let mut first = ProfileCollector::new();
        first.record_operator_profile(OperatorProfile {
            node_id: 7,
            partition_id: Some(0),
            name: "ScanVertices".to_string(),
            output_rows: 2,
            peak_memory_bytes: 10,
            ..OperatorProfile::default()
        });
        let mut second = ProfileCollector::new();
        second.record_operator_profile(OperatorProfile {
            node_id: 7,
            partition_id: Some(1),
            name: "ScanVertices".to_string(),
            output_rows: 3,
            peak_memory_bytes: 20,
            ..OperatorProfile::default()
        });

        let mut aggregate = ProfileCollector::new();
        aggregate.aggregate_partition_profiles(&[first, second]);

        assert_eq!(aggregate.operators.len(), 2);
        assert_eq!(
            aggregate
                .operators
                .get(&OperatorProfileKey::new(7, Some(0)))
                .expect("partition zero profile")
                .output_rows,
            2
        );
        assert_eq!(
            aggregate
                .operators
                .get(&OperatorProfileKey::new(7, Some(1)))
                .expect("partition one profile")
                .peak_memory_bytes,
            20
        );
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
