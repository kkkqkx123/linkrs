//! WAL sync policy

use std::time::{Duration, Instant};

use crate::core::wal::types::SyncPolicy;

/// Determine if a sync operation is needed based on the sync policy
pub(crate) fn should_sync(
    policy: &SyncPolicy,
    write_count: u64,
    last_sync_elapsed: Option<Duration>,
) -> bool {
    match policy {
        SyncPolicy::Never => false,
        SyncPolicy::EveryWrite => true,
        SyncPolicy::Periodic { interval_ms } => {
            let elapsed_ms = last_sync_elapsed
                .map(|d| d.as_millis() as u64)
                .unwrap_or(u64::MAX);
            elapsed_ms >= *interval_ms
        }
        SyncPolicy::Batch { batch_size } => write_count >= *batch_size as u64,
    }
}

/// Calculate elapsed since last sync
pub(crate) fn elapsed_since(last_sync_time: Option<Instant>) -> Option<Duration> {
    last_sync_time.map(|t| t.elapsed())
}
