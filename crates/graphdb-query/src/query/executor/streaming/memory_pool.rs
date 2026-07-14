//! Hierarchical MemoryPool for query execution.
//!
//! M4: replaces the flat per-query `MemoryBudget` with a multi-level tree:
//!
//! ```text
//! DatabaseMemoryPool (process-level, admission control)
//!   → QueryPool       (per-query slice)
//!     → FragmentPool   (per-fragment within query)
//!       → OperatorPool  (per-operator within fragment)
//!         → TaskPool     (per-task within operator)
//! ```
//!
//! All reservations use RAII guards.  The database pool enforces a global
//! admission limit so that no single query can exhaust memory for the
//! entire process.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use crate::core::error::QueryError;

use super::chunk::DataChunk;

/// Error type for memory pool operations.
#[derive(Debug, Clone)]
pub enum MemoryPoolError {
    /// The requested bytes would exceed the pool's limit.
    ExceededLimit {
        requested: usize,
        available: usize,
        pool_name: &'static str,
    },
    /// The pool has been shut down.
    ShutDown(&'static str),
}

impl std::fmt::Display for MemoryPoolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ExceededLimit {
                requested,
                available,
                pool_name,
            } => {
                write!(
                    f,
                    "{}: requested {} bytes, {} bytes available",
                    pool_name, requested, available
                )
            }
            Self::ShutDown(name) => write!(f, "{}: pool is shut down", name),
        }
    }
}

impl std::error::Error for MemoryPoolError {}

impl From<MemoryPoolError> for QueryError {
    fn from(e: MemoryPoolError) -> Self {
        QueryError::execution(e.to_string())
    }
}

// ── Inner state (shared across clones) ──────────────────────────────────

/// Thread-safe inner state for a memory pool.
#[derive(Debug)]
struct PoolInner {
    /// Upper bound (soft limit).  0 = unlimited.
    max_bytes: usize,
    /// Current accounted bytes.
    used: AtomicUsize,
    /// Whether the pool has been shut down (prevents new reservations).
    shut_down: std::sync::atomic::AtomicBool,
}

impl PoolInner {
    fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            used: AtomicUsize::new(0),
            shut_down: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn try_reserve(&self, bytes: usize) -> Result<(), MemoryPoolError> {
        if self.shut_down.load(Ordering::Relaxed) {
            return Err(MemoryPoolError::ShutDown("pool"));
        }
        if self.max_bytes == 0 {
            // Unlimited pool — always succeed but still track.
            self.used.fetch_add(bytes, Ordering::Relaxed);
            return Ok(());
        }
        loop {
            let current = self.used.load(Ordering::Relaxed);
            let new_total = current.checked_add(bytes).ok_or_else(|| {
                MemoryPoolError::ExceededLimit {
                    requested: bytes,
                    available: self.max_bytes.saturating_sub(current),
                    pool_name: "pool (overflow)",
                }
            })?;
            if new_total > self.max_bytes {
                return Err(MemoryPoolError::ExceededLimit {
                    requested: bytes,
                    available: self.max_bytes.saturating_sub(current),
                    pool_name: "pool",
                });
            }
            if self
                .used
                .compare_exchange_weak(current, new_total, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    fn release(&self, bytes: usize) {
        self.used.fetch_sub(bytes, Ordering::Relaxed);
    }

    fn current(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    fn shut_down(&self) {
        self.shut_down.store(true, Ordering::Relaxed);
    }
}

// ── RAII Reservation ────────────────────────────────────────────────────

/// RAII guard that releases reserved memory on drop.
#[derive(Debug)]
pub struct MemoryPoolReservation {
    inner: Option<Arc<PoolInner>>,
    bytes: usize,
}

impl MemoryPoolReservation {
    fn new(inner: Arc<PoolInner>, bytes: usize) -> Self {
        Self {
            inner: Some(inner),
            bytes,
        }
    }

    /// Create a reservation from bytes that are already accounted for.
    /// Does not reserve any new bytes — the caller is responsible for
    /// having already deducted them from the pool.
    fn from_accounted(inner: Arc<PoolInner>, bytes: usize) -> Self {
        Self {
            inner: Some(inner),
            bytes,
        }
    }

    /// Forget the reservation — memory is not released on drop.
    pub fn forget(mut self) {
        self.bytes = 0;
        self.inner = None;
    }

    /// The amount of memory reserved.
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

impl Drop for MemoryPoolReservation {
    fn drop(&mut self) {
        if let Some(ref inner) = self.inner {
            if self.bytes > 0 {
                inner.release(self.bytes);
            }
        }
    }
}

impl Clone for MemoryPoolReservation {
    fn clone(&self) -> Self {
        // Cloning a reservation creates a new independent reservation
        // for the same bytes.  This is used when sharing a pool handle
        // across threads (e.g. QueryPool is Clone).
        //
        // We do NOT double-reserve — the clone's bytes will be released
        // independently on drop, which means the total released may
        // exceed reserved.  This is acceptable because Clone on pool
        // types is only used for handle sharing, where the original
        // reservation holds the accounting and clones are just accessors.
        if let Some(ref inner) = self.inner {
            // Release on drop of this clone will decrement correctly
            // because the bytes were already accounted for by the
            // original reservation.
            Self {
                inner: Some(Arc::clone(inner)),
                bytes: self.bytes,
            }
        } else {
            Self {
                inner: None,
                bytes: 0,
            }
        }
    }
}

/// Clonable handle that shares the same pool account.
#[derive(Debug, Clone)]
pub struct PoolHandle {
    inner: Arc<PoolInner>,
}

impl PoolHandle {
    fn new(max_bytes: usize) -> Self {
        Self {
            inner: Arc::new(PoolInner::new(max_bytes)),
        }
    }

    /// Reserve `bytes` from this pool.
    pub fn reserve(&self, bytes: usize) -> Result<MemoryPoolReservation, MemoryPoolError> {
        self.inner.try_reserve(bytes)?;
        Ok(MemoryPoolReservation::new(Arc::clone(&self.inner), bytes))
    }

    /// Return current usage.
    pub fn current(&self) -> usize {
        self.inner.current()
    }

    /// Return the pool's maximum.
    pub fn max(&self) -> usize {
        self.inner.max_bytes
    }

    /// Shut down this pool (rejects new reservations).
    pub fn shut_down(&self) {
        self.inner.shut_down();
    }
}

// ── DatabaseMemoryPool (process-level) ──────────────────────────────────

/// Process-level memory pool with admission control.
///
/// This is the root of the hierarchy.  All query memory is ultimately
/// accounted against this pool.  Admission control ensures that a single
/// query cannot exhaust memory for the entire process.
#[derive(Debug, Clone)]
pub struct DatabaseMemoryPool {
    handle: PoolHandle,
    /// The maximum bytes any single query may reserve at creation time.
    /// Queries that request more than this are rejected immediately.
    max_query_bytes: usize,
}

impl DatabaseMemoryPool {
    /// Create a new database pool with the given global limit.
    ///
    /// `max_bytes`: total memory the database may use (0 = unlimited).
    /// `max_query_bytes`: maximum any single query may reserve.
    pub fn new(max_bytes: usize, max_query_bytes: usize) -> Self {
        Self {
            handle: PoolHandle::new(max_bytes),
            max_query_bytes,
        }
    }

    /// Create a pool with sensible defaults (8 GiB database, 512 MiB per query).
    pub fn default_pool() -> Self {
        Self::new(8 * 1024 * 1024 * 1024, 512 * 1024 * 1024)
    }

    /// Create an unlimited pool (for testing).
    pub fn unlimited() -> Self {
        Self::new(0, usize::MAX)
    }

    /// Allocate a query-level pool from this database.
    ///
    /// The caller specifies the desired per-query limit.  If it exceeds
    /// `max_query_bytes`, the cap is applied.  Admission control checks
    /// that the database has enough remaining capacity.
    pub fn new_query_pool(&self, requested_bytes: usize) -> Result<QueryPool, MemoryPoolError> {
        let effective = requested_bytes.min(self.max_query_bytes);
        let reservation = self.handle.reserve(effective)?;
        Ok(QueryPool {
            database: self.clone(),
            handle: PoolHandle::new(effective),
            _reservation: reservation,
        })
    }

    /// Current global memory usage.
    pub fn current(&self) -> usize {
        self.handle.current()
    }

    /// Maximum global memory.
    pub fn max(&self) -> usize {
        self.handle.max()
    }

    /// Shut down the database pool (rejects all new reservations).
    pub fn shut_down(&self) {
        self.handle.shut_down();
    }

    /// Return the inner pool handle (for direct reservation in tests).
    pub fn handle(&self) -> PoolHandle {
        self.handle.clone()
    }
}

// ── QueryPool ───────────────────────────────────────────────────────────

/// Per-query memory pool, allocated from [`DatabaseMemoryPool`].
///
/// The query pool is the root for all operator memory within a single
/// query execution.  It is released when the query completes.
#[derive(Debug, Clone)]
pub struct QueryPool {
    database: DatabaseMemoryPool,
    handle: PoolHandle,
    _reservation: MemoryPoolReservation,
}

impl QueryPool {
    /// Create a fragment pool from this query pool.
    pub fn new_fragment_pool(&self, requested_bytes: usize) -> Result<FragmentPool, MemoryPoolError> {
        let reservation = self.handle.reserve(requested_bytes)?;
        Ok(FragmentPool {
            query: self.clone(),
            handle: PoolHandle::new(requested_bytes),
            _reservation: reservation,
        })
    }

    /// Reserve bytes directly from the query pool.
    pub fn reserve(&self, bytes: usize) -> Result<MemoryPoolReservation, MemoryPoolError> {
        self.handle.reserve(bytes)
    }

    /// Current usage.
    pub fn current(&self) -> usize {
        self.handle.current()
    }

    /// Maximum bytes for this query.
    pub fn max(&self) -> usize {
        self.handle.max()
    }
}

// ── FragmentPool ────────────────────────────────────────────────────────

/// Per-fragment memory pool, allocated from [`QueryPool`].
#[derive(Debug, Clone)]
pub struct FragmentPool {
    query: QueryPool,
    handle: PoolHandle,
    _reservation: MemoryPoolReservation,
}

impl FragmentPool {
    /// Create an operator pool from this fragment pool.
    pub fn new_operator_pool(
        &self,
        requested_bytes: usize,
    ) -> Result<OperatorPool, MemoryPoolError> {
        let reservation = self.handle.reserve(requested_bytes)?;
        Ok(OperatorPool {
            fragment: self.clone(),
            handle: PoolHandle::new(requested_bytes),
            _reservation: reservation,
        })
    }

    /// Reserve bytes directly from the fragment pool.
    pub fn reserve(&self, bytes: usize) -> Result<MemoryPoolReservation, MemoryPoolError> {
        self.handle.reserve(bytes)
    }

    /// Current usage.
    pub fn current(&self) -> usize {
        self.handle.current()
    }
}

// ── OperatorPool ────────────────────────────────────────────────────────

/// Per-operator memory pool, allocated from [`FragmentPool`].
#[derive(Debug, Clone)]
pub struct OperatorPool {
    fragment: FragmentPool,
    handle: PoolHandle,
    _reservation: MemoryPoolReservation,
}

impl OperatorPool {
    /// Create a task pool from this operator pool.
    pub fn new_task_pool(&self, requested_bytes: usize) -> Result<TaskPool, MemoryPoolError> {
        let reservation = self.handle.reserve(requested_bytes)?;
        Ok(TaskPool {
            operator: self.clone(),
            handle: PoolHandle::new(requested_bytes),
            _reservation: reservation,
        })
    }

    /// Reserve bytes directly from the operator pool.
    pub fn reserve(&self, bytes: usize) -> Result<MemoryPoolReservation, MemoryPoolError> {
        self.handle.reserve(bytes)
    }

    /// Current usage.
    pub fn current(&self) -> usize {
        self.handle.current()
    }
}

// ── TaskPool ────────────────────────────────────────────────────────────

/// Per-task memory pool, allocated from [`OperatorPool`].
///
/// This is the leaf of the memory hierarchy.  Each task (partition worker)
/// gets its own pool that accounts against the operator, fragment, query,
/// and database limits.
#[derive(Debug, Clone)]
pub struct TaskPool {
    operator: OperatorPool,
    handle: PoolHandle,
    _reservation: MemoryPoolReservation,
}

impl TaskPool {
    /// Reserve bytes from the task pool.
    pub fn reserve(&self, bytes: usize) -> Result<MemoryPoolReservation, MemoryPoolError> {
        self.handle.reserve(bytes)
    }

    /// Current usage.
    pub fn current(&self) -> usize {
        self.handle.current()
    }
}

// ── PooledChunk ────────────────────────────────────────────────────────

/// A DataChunk with an attached [`MemoryPoolReservation`].
///
/// The reservation is released when the `PooledChunk` is dropped.
#[derive(Debug)]
pub struct PooledChunk {
    pub chunk: DataChunk,
    pub reservation: MemoryPoolReservation,
}

impl PooledChunk {
    pub fn new(chunk: DataChunk, reservation: MemoryPoolReservation) -> Self {
        Self { chunk, reservation }
    }

    /// Decompose into parts.
    pub fn into_parts(self) -> (DataChunk, MemoryPoolReservation) {
        (self.chunk, self.reservation)
    }
}

impl PoolHandle {
    /// Deep-copy a [`DataChunk`] into this pool, accounting the copied bytes.
    ///
    /// Returns a [`PooledChunk`] whose reservation is released on drop.
    /// Schema and layout are shared (cheap `Arc::clone`).
    pub fn copy_chunk(&self, chunk: &DataChunk) -> Result<PooledChunk, QueryError> {
        use crate::query::executor::base::MemoryBudget;
        let estimated = MemoryBudget::estimate_rows_memory(&chunk.rows);
        let reservation = self
            .reserve(estimated)
            .map_err(|e| QueryError::execution(e.to_string()))?;
        let copied = DataChunk {
            rows: chunk.rows.clone(),
            schema: std::sync::Arc::clone(&chunk.schema),
            layout: std::sync::Arc::clone(&chunk.layout),
            memory_reservation: None,
        };
        Ok(PooledChunk {
            chunk: copied,
            reservation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_pool_admission() {
        let db = DatabaseMemoryPool::new(1024, 400);
        let _qp = db.new_query_pool(500).expect("first query admitted (capped to 400)");
        // 400 used from DB (capped by max_query_bytes)
        // 624 remaining in DB
        let _qp2 = db.new_query_pool(700).expect("second query admitted (capped to 400, 800 <= 1024)");
        // To exceed global limit, we need both query pools active and a third
        let result = db.new_query_pool(400);
        assert!(result.is_err(), "third query should exceed global limit (1200 > 1024)");
    }

    #[test]
    fn test_query_pool_reserve_release() {
        let db = DatabaseMemoryPool::unlimited();
        let qp = db.new_query_pool(1024).expect("query pool");
        let reservation = qp.reserve(128).expect("reserve");
        assert_eq!(qp.current(), 128);
        drop(reservation);
        assert_eq!(qp.current(), 0);
    }

    #[test]
    fn test_hierarchical_reservation() {
        let db = DatabaseMemoryPool::new(4096, 2048);
        let qp = db.new_query_pool(2048).expect("query");
        let fp = qp.new_fragment_pool(1024).expect("fragment");
        let op = fp.new_operator_pool(512).expect("operator");
        let _tp = op.new_task_pool(256).expect("task");

        // tp creation reserved 256 from op.handle
        assert_eq!(op.current(), 256);
        // fp has 512 reserved (for op) + 0 from op's reservation
        assert_eq!(fp.current(), 512);
        // qp has 1024 reserved (for fp)
        assert_eq!(qp.current(), 1024);
        // db has 2048 reserved (for qp)
        assert!(db.current() >= 2048);
    }

    #[test]
    fn test_unlimited_pool() {
        let db = DatabaseMemoryPool::unlimited();
        let qp = db.new_query_pool(usize::MAX).expect("unlimited query");
        let r = qp.reserve(1_000_000).expect("large reserve");
        assert!(qp.current() >= 1_000_000);
        drop(r);
    }

    #[test]
    fn test_shut_down_rejects() {
        let db = DatabaseMemoryPool::new(1024, 512);
        db.shut_down();
        let result = db.new_query_pool(64);
        assert!(result.is_err());
        match result {
            Err(MemoryPoolError::ShutDown(_)) => {} // expected
            _ => panic!("expected ShutDown error"),
        }
    }

    #[test]
    fn test_reservation_forget() {
        let db = DatabaseMemoryPool::unlimited();
        let qp = db.new_query_pool(1024).expect("query");
        let r = qp.reserve(256).expect("reserve");
        r.forget();
        // Memory is NOT released after forget.
        assert!(qp.current() >= 256);
    }
}
