//! WAL sync policy

use std::time::{Duration, Instant};

/// Calculate elapsed since last sync
pub(crate) fn elapsed_since(last_sync_time: Option<Instant>) -> Option<Duration> {
    last_sync_time.map(|t| t.elapsed())
}
