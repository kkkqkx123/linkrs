//! Bulk-load session: suspend mid-load checkpoint/flush triggers, then run
//! exactly one final checkpoint.
//!
//! Without this, a large auto-commit ingest fires a checkpoint every
//! `checkpoint_threshold` WAL bytes. Each of those checkpoints flushes the
//! **full** dataset (the incremental/shadow-page path is defeated during a
//! bulk load: the first checkpoint has no baseline, and a slow checkpoint
//! ages the baseline past `MAX_BASELINE_AGE_MS`, cascading further full
//! rewrites). The k-th checkpoint therefore rewrites ~k·threshold records
//! and the total flush cost is amortized **O(n²)** in the number of loaded
//! statements. A 20100-statement load measured ~194s this way, versus ~28s
//! with checkpoints suspended (linear, ~1.4 ms/statement).
//!
//! A session suppresses threshold/time-based checkpoint and flush triggers
//! (see `PersistenceCoordinator::should_checkpoint` / `should_flush`) while
//! keeping the hard `max_wal_size` safety valve active, and performs exactly
//! one final checkpoint in [`BulkLoadSession::finish`].

use std::sync::Arc;

use linkrs_core::StorageResult;

use crate::engine::graph_storage::context::GraphStorageContext;
use crate::engine::persistence_coordinator::{CheckpointStats, PersistenceCoordinator};

/// Guard for one bulk-load session, created by
/// [`crate::GraphStorage::begin_bulk_load`].
///
/// While the session is alive, threshold/time-based checkpoint and flush
/// triggers are suspended. Call [`BulkLoadSession::finish`] to lift the
/// suspension and run the single final checkpoint; dropping the session
/// without finishing still lifts the suspension (no checkpoint), so an
/// aborted load never leaves the storage muted.
pub struct BulkLoadSession {
    coordinator: Option<Arc<parking_lot::RwLock<PersistenceCoordinator>>>,
    ctx: Arc<GraphStorageContext>,
    finished: bool,
}

impl BulkLoadSession {
    pub(crate) fn new(
        coordinator: Option<Arc<parking_lot::RwLock<PersistenceCoordinator>>>,
        ctx: Arc<GraphStorageContext>,
    ) -> Self {
        if let Some(persistence) = &coordinator {
            persistence.read().enter_bulk_load();
        }
        Self {
            coordinator,
            ctx,
            finished: false,
        }
    }

    /// Whether checkpoint/flush suspension is currently engaged.
    pub fn active(&self) -> bool {
        self.coordinator
            .as_ref()
            .is_some_and(|persistence| persistence.read().bulk_load_active())
    }

    /// Lift this session's suspension reference and run exactly one final
    /// checkpoint so the whole ingest is durably anchored in a single flush.
    ///
    /// If other bulk-load sessions are still active, suspension stays engaged
    /// for them; the explicit final checkpoint runs regardless.
    ///
    /// In-memory storage has no persistence layer; the session is a no-op and
    /// `Ok(None)` is returned.
    pub fn finish(mut self) -> StorageResult<Option<CheckpointStats>> {
        let Some(persistence) = self.release() else {
            self.finished = true;
            return Ok(None);
        };
        self.finished = true;
        drop(persistence);
        crate::engine::graph_storage::persistence::create_checkpoint(&self.ctx)
    }

    /// Lift this session's suspension reference without running the final
    /// checkpoint. Data loaded so far stays recoverable through WAL replay.
    pub fn abandon(mut self) {
        self.release();
        self.finished = true;
    }

    /// Release the suspension reference and consume the coordinator handle.
    /// Returns `None` when there is no persistence layer or the session was
    /// already released; each session releases exactly once because the
    /// handle is taken out of `self`.
    fn release(&mut self) -> Option<Arc<parking_lot::RwLock<PersistenceCoordinator>>> {
        let persistence = self.coordinator.take()?;
        persistence.read().exit_bulk_load();
        Some(persistence)
    }
}

impl Drop for BulkLoadSession {
    fn drop(&mut self) {
        if !self.finished {
            self.release();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::PersistenceConfig;
    use tempfile::TempDir;

    fn test_coordinator() -> Arc<parking_lot::RwLock<PersistenceCoordinator>> {
        let temp_dir = TempDir::new().expect("temp dir");
        let config = PersistenceConfig::for_work_dir(temp_dir.path());
        Arc::new(parking_lot::RwLock::new(
            PersistenceCoordinator::new(config).expect("coordinator"),
        ))
    }

    /// In-memory context; the session only holds it for `finish`, which the
    /// release-behavior tests below never call (the final-checkpoint path is
    /// covered by the e2e perf tests on a full persistent storage).
    fn ctx() -> Arc<GraphStorageContext> {
        Arc::new(GraphStorageContext::new())
    }

    #[test]
    fn overlapping_sessions_keep_suspension_until_last_exits() {
        let persistence = test_coordinator();

        let session_a = BulkLoadSession::new(Some(persistence.clone()), ctx());
        let session_b = BulkLoadSession::new(Some(persistence.clone()), ctx());
        assert!(session_a.active());
        assert!(session_b.active());

        // The first session exiting must not un-suspend the other one.
        session_a.abandon();
        assert!(
            persistence.read().bulk_load_active(),
            "suspension must persist while another session is active"
        );

        drop(session_b);
        assert!(
            !persistence.read().bulk_load_active(),
            "suspension lifts only after the last session exits"
        );
    }

    #[test]
    fn session_count_is_reentrant_per_session() {
        let persistence = test_coordinator();

        for _ in 0..3 {
            let session = BulkLoadSession::new(Some(persistence.clone()), ctx());
            assert!(persistence.read().bulk_load_active());
            session.abandon();
            assert!(!persistence.read().bulk_load_active());
        }
    }

    #[test]
    fn unfinished_session_releases_on_drop() {
        let persistence = test_coordinator();

        {
            let _session = BulkLoadSession::new(Some(persistence.clone()), ctx());
            assert!(persistence.read().bulk_load_active());
        }
        assert!(
            !persistence.read().bulk_load_active(),
            "dropping without finish/abandon must still release the reference"
        );
    }

    #[test]
    fn session_without_persistence_is_noop() {
        let session = BulkLoadSession::new(None, ctx());
        assert!(!session.active());
        session.abandon();
    }

    /// `finish` needs a real `GraphStorageContext` to run the final
    /// checkpoint, which requires a full persistent storage setup covered by
    /// the e2e perf tests; reference-count release is asserted above.
    fn ctx() -> Arc<GraphStorageContext> {
        unreachable!("ctx is only touched by finish(), which these tests avoid")
    }
}
