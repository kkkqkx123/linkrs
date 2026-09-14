//! Runtime resource lifecycle.
//!
//! [`ResourceOwner`] accumulates cleanup callbacks (cursors, temp files,
//! spill reservations) released together at query teardown.
//! [`QueryFinishGuard`] is an RAII guard that marks a query finished in the
//! [`QueryManager`] on drop, covering explicit-finish omission and panics.

use std::sync::Arc;

use crate::query_manager::QueryManager;

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

/// RAII guard that marks a query as finished in the QueryManager on drop.
///
/// Created by [`crate::executor::streaming::runtime::ExecutionRuntime::finish_guard`].
/// Ensures the query lifecycle is tracked even when the caller forgets to
/// call finish explicitly, or when execution panics mid-flight.
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
