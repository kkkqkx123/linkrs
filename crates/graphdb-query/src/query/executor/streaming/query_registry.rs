//! Query registry — process-level query identity and cancellation.
//!
//! Every query execution must first obtain a unique, non-zero `QueryId` from
//! the [`QueryRegistry`].  The registry also holds the unified cancellation
//! source, deadline, and weak handles so that `KILL QUERY` works regardless
//! of which layer initiated cancellation.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;

use super::transaction_scope::CancelReason;

// ── QueryId ─────────────────────────────────────────────────────────────────

/// Typed, non-zero query identifier.
///
/// Allocated by [`QueryRegistry::register`].  Zero is never a valid id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QueryId(pub u64);

impl std::fmt::Display for QueryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Q{}", self.0)
    }
}

impl QueryId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn is_zero(&self) -> bool {
        self.0 == 0
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn as_i64(&self) -> i64 {
        self.0 as i64
    }
}

// ── QueryMetadata ───────────────────────────────────────────────────────────

/// Metadata stored in the registry for each active query.
#[derive(Debug, Clone)]
pub struct QueryMetadata {
    pub query_id: QueryId,
    pub session_id: Option<i64>,
    pub user_name: Option<String>,
    pub space_name: Option<String>,
    pub query_text: Option<String>,
    pub start_time: Instant,
}

// ── QueryRegistry ───────────────────────────────────────────────────────────

/// Process-level query registry.
///
/// - Allocates unique, non-zero `QueryId` values.
/// - Tracks active queries with metadata and weak runtime handles.
/// - Provides unified cancellation through a per-query `CancelReason`.
/// - Responsible for clean teardown: once teardown completes, the query
///   is removed from the registry.
#[derive(Debug)]
pub struct QueryRegistry {
    next_id: AtomicU64,
    active: DashMap<QueryId, QueryEntry>,
}

#[derive(Debug)]
struct QueryEntry {
    _metadata: QueryMetadata,
    cancel_reason: Arc<parking_lot::Mutex<Option<CancelReason>>>,
}

impl QueryRegistry {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            active: DashMap::new(),
        }
    }

    /// Register a new query and obtain its unique `QueryId`.
    ///
    /// The caller should later call [`Self::unregister`] once the query
    /// lifecycle ends (after teardown).
    pub fn register(&self, metadata: QueryMetadata) -> (QueryId, CancelToken) {
        let id = self.allocate_id();
        let reason = Arc::new(parking_lot::Mutex::new(None));
        let token = CancelToken(reason.clone());

        self.active.insert(id, QueryEntry {
            _metadata: QueryMetadata {
                query_id: id,
                ..metadata
            },
            cancel_reason: reason,
        });

        (id, token)
    }

    /// Unregister a completed query.
    ///
    /// Returns `true` if the query was actually registered, `false` if it
    /// was already unknown (harmless — may happen after forced removal).
    pub fn unregister(&self, query_id: QueryId) -> bool {
        self.active.remove(&query_id).is_some()
    }

    /// Cancel a query with a reason.
    ///
    /// Returns the first reason that was set (idempotent — subsequent calls
    /// for the same query return `None`).
    pub fn cancel(&self, query_id: QueryId, reason: CancelReason) -> Option<CancelReason> {
        if let Some(entry) = self.active.get(&query_id) {
            let mut slot = entry.cancel_reason.lock();
            if slot.is_some() {
                None // first reason wins
            } else {
                *slot = Some(reason);
                slot.clone()
            }
        } else {
            None
        }
    }

    /// Cancel all active queries with a given reason (used on shutdown).
    pub fn cancel_all(&self, reason: CancelReason) -> Vec<QueryId> {
        let ids: Vec<QueryId> = self.active.iter().map(|e| *e.key()).collect();
        for id in &ids {
            self.cancel(*id, reason.clone());
        }
        ids
    }

    /// Check whether a query has been cancelled and get the reason.
    pub fn cancellation_reason(&self, query_id: QueryId) -> Option<CancelReason> {
        self.active
            .get(&query_id)
            .and_then(|e| e.cancel_reason.lock().clone())
    }

    /// Number of currently active queries.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Iterate over all active query IDs.
    pub fn active_queries(&self) -> Vec<QueryId> {
        self.active.iter().map(|e| *e.key()).collect()
    }

    fn allocate_id(&self) -> QueryId {
        loop {
            let raw = self.next_id.fetch_add(1, Ordering::Relaxed);
            if raw != 0 {
                return QueryId(raw);
            }
        }
    }
}

impl Default for QueryRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── CancelToken ─────────────────────────────────────────────────────────────

/// Thread-safe token that tracks whether a query was cancelled and why.
///
/// Shared with [`ExecutionRuntime`](super::runtime::ExecutionRuntime) and
/// checked by operators during execution.
#[derive(Debug, Clone)]
pub struct CancelToken(Arc<parking_lot::Mutex<Option<CancelReason>>>);

impl CancelToken {
    pub fn new() -> Self {
        Self(Arc::new(parking_lot::Mutex::new(None)))
    }

    /// Mark as cancelled.  Returns the _first_ reason that was set, or
    /// `None` if already cancelled.
    pub fn cancel(&self, reason: CancelReason) -> Option<CancelReason> {
        let mut slot = self.0.lock();
        if slot.is_some() {
            None
        } else {
            *slot = Some(reason);
            slot.clone()
        }
    }

    /// Check whether cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.0.lock().is_some()
    }

    /// Get the cancellation reason, if any.
    pub fn reason(&self) -> Option<CancelReason> {
        self.0.lock().clone()
    }

    /// Clear the cancellation state (for reuse).
    pub fn clear(&self) {
        *self.0.lock() = None;
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

// ── QueryGuard ──────────────────────────────────────────────────────────────

/// RAII guard that unregisters a query from the registry on drop.
#[derive(Debug)]
pub struct QueryGuard {
    registry: Option<Arc<QueryRegistry>>,
    query_id: QueryId,
    finished: bool,
}

impl QueryGuard {
    pub fn new(registry: Arc<QueryRegistry>, query_id: QueryId) -> Self {
        Self {
            registry: Some(registry),
            query_id,
            finished: false,
        }
    }

    /// Unregister immediately without waiting for drop.
    pub fn finish(&mut self) {
        if !self.finished {
            self.finished = true;
            if let Some(ref reg) = self.registry {
                reg.unregister(self.query_id);
            }
        }
    }
}

impl Drop for QueryGuard {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(ref reg) = self.registry {
                reg.unregister(self.query_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_register_and_unregister() {
        let reg = Arc::new(QueryRegistry::new());
        let meta = QueryMetadata {
            query_id: QueryId(0),
            session_id: Some(1),
            user_name: Some("test".to_string()),
            space_name: None,
            query_text: Some("MATCH (n) RETURN n".to_string()),
            start_time: Instant::now(),
        };
        let (id, _) = reg.register(meta);
        assert!(!id.is_zero());
        assert_eq!(reg.active_count(), 1);

        assert!(reg.unregister(id));
        assert_eq!(reg.active_count(), 0);
    }

    #[test]
    fn test_cancel_token() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());

        let first = token.cancel(CancelReason::UserKill);
        assert_eq!(first, Some(CancelReason::UserKill));
        assert!(token.is_cancelled());

        let second = token.cancel(CancelReason::Deadline);
        assert!(second.is_none());
        assert_eq!(token.reason(), Some(CancelReason::UserKill));
    }

    #[test]
    fn test_cancel_all() {
        let reg = Arc::new(QueryRegistry::new());
        let meta1 = QueryMetadata {
            query_id: QueryId(0),
            session_id: None,
            user_name: None,
            space_name: None,
            query_text: None,
            start_time: Instant::now(),
        };
        let meta2 = meta1.clone();
        reg.register(meta1);
        reg.register(meta2);

        let cancelled = reg.cancel_all(CancelReason::Shutdown);
        assert_eq!(cancelled.len(), 2);
        assert_eq!(reg.active_count(), 2);

        for id in &cancelled {
            assert_eq!(
                reg.cancellation_reason(*id),
                Some(CancelReason::Shutdown)
            );
        }
    }

    #[test]
    fn test_query_guard_drop() {
        let reg = Arc::new(QueryRegistry::new());
        let meta = QueryMetadata {
            query_id: QueryId(0),
            session_id: None,
            user_name: None,
            space_name: None,
            query_text: None,
            start_time: Instant::now(),
        };
        let (id, _) = reg.register(meta);
        {
            let _guard = QueryGuard::new(reg.clone(), id);
            assert_eq!(reg.active_count(), 1);
        }
        assert_eq!(reg.active_count(), 0);
    }

    #[test]
    fn test_query_guard_finish() {
        let reg = Arc::new(QueryRegistry::new());
        let meta = QueryMetadata {
            query_id: QueryId(0),
            session_id: None,
            user_name: None,
            space_name: None,
            query_text: None,
            start_time: Instant::now(),
        };
        let (id, _) = reg.register(meta);
        let mut guard = QueryGuard::new(reg.clone(), id);
        guard.finish();
        assert_eq!(reg.active_count(), 0);
        // Drop after finish is safe (no double-unregister)
    }

    #[test]
    fn test_ids_are_unique_and_non_zero() {
        let reg = QueryRegistry::new();
        let mut ids = std::collections::HashSet::new();
        for _ in 0..100 {
            let meta = QueryMetadata {
                query_id: QueryId(0),
                session_id: None,
                user_name: None,
                space_name: None,
                query_text: None,
                start_time: Instant::now(),
            };
            let (id, _) = reg.register(meta);
            assert!(!id.is_zero());
            assert!(ids.insert(id));
        }
    }
}
