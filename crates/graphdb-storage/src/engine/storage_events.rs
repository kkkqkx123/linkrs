use std::sync::Arc;

/// Storage engine lifecycle notification.
///
/// Emission is best-effort and synchronous on the persistence path.
/// Observers must return quickly and must not call back into the
/// persistence coordinator while handling an event.
#[derive(Debug, Clone)]
pub enum StorageEvent {
    CheckpointStarted {
        sequence: u64,
    },
    CheckpointCompleted {
        sequence: u64,
        duration_ms: u64,
        data_flushed: u64,
        wal_truncated: u64,
        snapshot_created: bool,
    },
    CheckpointFailed {
        sequence: Option<u64>,
        error: String,
    },
    WalTruncated {
        up_to_lsn: u64,
    },
    SnapshotCreated {
        sequence: u64,
    },
    GcRun {
        reclaimed_entries: u64,
    },
    CompactionStarted {
        space_id: u64,
    },
    CompactionCompleted {
        space_id: u64,
        reclaimed_bytes: u64,
    },
}

/// Sentinel space id used for global (cross-space) transactional compaction.
///
/// Compaction operates on the whole store rather than a single space, so
/// observers see this id in `CompactionStarted` / `CompactionCompleted`.
pub const GLOBAL_COMPACTION_SPACE_ID: u64 = 0;

/// Runtime observer for storage events.
pub type StorageEventCallback = Arc<dyn Fn(&StorageEvent) + Send + Sync>;

/// Observer for reclaimed-entry counts, forwarded by GC managers to the
/// persistence coordinator which fans out `StorageEvent::GcRun`.
pub type GcEventSink = Arc<dyn Fn(u64) + Send + Sync>;
