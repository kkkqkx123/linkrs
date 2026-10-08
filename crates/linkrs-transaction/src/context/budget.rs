//! Transaction context behavior: budgets, staged bytes and commit metadata

use std::sync::atomic::Ordering;

use super::TransactionContext;
use crate::types::*;
use linkrs_core::types::CommitLsn;
impl TransactionContext {
    /// Drain pending budget warnings queued during mutation recording.
    ///
    /// Exactly-once: each warning is returned at most once. The manager
    /// fans drained events out through the commit observers on the commit path.
    pub fn drain_budget_warnings(&self) -> Vec<TransactionEvent> {
        std::mem::take(&mut *self.budget_warnings.lock())
    }

    /// Number of pending budget warnings not yet drained.
    pub fn pending_budget_warnings(&self) -> usize {
        self.budget_warnings.lock().len()
    }

    pub fn staged_bytes(&self) -> u64 {
        self.staged_bytes.load(Ordering::Relaxed)
    }

    pub fn mark_commit_published(&self, commit_lsn: CommitLsn) {
        self.commit_lsn.store(commit_lsn.get(), Ordering::Release);
        self.commit_published.store(true);
    }

    pub fn commit_published(&self) -> bool {
        self.commit_published.load()
    }

    pub fn commit_lsn(&self) -> CommitLsn {
        CommitLsn::new(self.commit_lsn.load(Ordering::Acquire))
    }

    pub fn add_staged_bytes(&self, bytes: u64) {
        self.staged_bytes.fetch_add(bytes, Ordering::Relaxed);
    }

    /// Increment query count
    pub fn increment_query_count(&self) {
        self.query_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Get query count
    pub fn query_count(&self) -> u64 {
        self.query_count.load(Ordering::Relaxed)
    }

    /// Get the current schema catalog version for this transaction.
    /// The version is incremented on every DDL operation and can be used by the
    /// query layer to detect schema changes and invalidate stale plan caches.
    pub fn schema_catalog_version(&self) -> u64 {
        self.schema_catalog_version.load(Ordering::Relaxed)
    }

    /// Increment the schema catalog version after a DDL operation.
    /// Returns the new version.
    pub fn bump_schema_catalog_version(&self) -> u64 {
        self.schema_catalog_version.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub fn set_pessimistic_lock(&self) {
        self.pessimistic_lock_held.store(true);
    }

    pub fn has_pessimistic_lock(&self) -> bool {
        self.pessimistic_lock_held.load()
    }

    pub fn clear_pessimistic_lock(&self) {
        self.pessimistic_lock_held.store(false);
    }
}
