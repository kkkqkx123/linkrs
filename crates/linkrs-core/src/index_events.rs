use std::sync::Arc;

/// Rebuild phase for progress reporting and crash diagnosis. Shared by the
/// fulltext and vector online rebuild drivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebuildPhase {
    Preparing,
    Backfilling,
    CatchingUp,
    Publishing,
    Completed,
    Failed,
}

impl RebuildPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            RebuildPhase::Preparing => "preparing",
            RebuildPhase::Backfilling => "backfilling",
            RebuildPhase::CatchingUp => "catching_up",
            RebuildPhase::Publishing => "publishing",
            RebuildPhase::Completed => "completed",
            RebuildPhase::Failed => "failed",
        }
    }
}

/// Mutable rebuild counters owned by the index managers and updated by the
/// sync-side rebuild drivers. `docs_*` count documents for fulltext and
/// point operations for vector rebuilds.
#[derive(Debug, Clone)]
pub struct RebuildProgress {
    pub space_id: u64,
    pub tag_name: String,
    pub field_name: String,
    pub generation: u64,
    pub phase: RebuildPhase,
    pub docs_scanned: u64,
    pub docs_applied: u64,
    pub docs_skipped: u64,
    pub started_at: chrono::DateTime<chrono::Utc>,
}

/// Fulltext/vector index lifecycle notification.
///
/// Emission is best-effort and synchronous on the index management path.
/// Observers must return quickly and must not call back into the index
/// manager while handling an event.
#[derive(Debug, Clone)]
pub enum IndexEvent {
    FulltextBuildStarted {
        index_name: String,
    },
    FulltextBuildCompleted {
        index_name: String,
        docs_count: u64,
    },
    FulltextRefresh {
        index_name: String,
    },
    FulltextDropped {
        index_name: String,
    },
    FulltextRebuildStarted {
        index_name: String,
        generation: u64,
    },
    /// Emitted on rebuild phase transitions (not per batch) with running
    /// counters; per-batch progress is queryable via rebuild progress APIs.
    FulltextRebuildProgress {
        index_name: String,
        generation: u64,
        phase: String,
        docs_applied: u64,
    },
    FulltextRebuildCompleted {
        index_name: String,
        generation: u64,
        docs_count: u64,
    },
    FulltextRebuildFailed {
        index_name: String,
        generation: u64,
        reason: String,
    },
    VectorBuildStarted {
        index_name: String,
    },
    VectorBuildCompleted {
        index_name: String,
        vectors_count: u64,
    },
    VectorRebuildStarted {
        index_name: String,
        generation: u64,
    },
    /// Emitted on vector rebuild phase transitions (not per batch).
    VectorRebuildProgress {
        index_name: String,
        generation: u64,
        phase: String,
        vectors_applied: u64,
    },
    VectorRebuildCompleted {
        index_name: String,
        generation: u64,
        vectors_count: u64,
    },
    VectorRebuildFailed {
        index_name: String,
        generation: u64,
        reason: String,
    },
    VectorDropped {
        index_name: String,
    },
    IndexMergeStarted {
        index_name: String,
        segments: usize,
    },
    IndexMergeCompleted {
        index_name: String,
    },
}

/// Runtime observer for index lifecycle events.
pub type IndexEventCallback = Arc<dyn Fn(&IndexEvent) + Send + Sync>;
