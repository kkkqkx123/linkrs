//! MVCC pure types: sentinels, errors, configuration and slot states.

use std::time::Duration;

use linkrs_core::types::Timestamp;

/// Released timestamp sentinel value (0 means timestamp has been released)
/// Note: distinct from Timestamp::MAX which may be used as a sentinel elsewhere
pub const RELEASED_TIMESTAMP: Timestamp = 0;

#[derive(Debug, Clone, thiserror::Error)]
pub enum VersionManagerError {
    #[error("Too many concurrent transactions")]
    TooManyTransactions,

    #[error("Invalid timestamp: {0}")]
    InvalidTimestamp(Timestamp),

    #[error("Timeout waiting for transaction")]
    Timeout,

    #[error("Failed to track snapshot for timestamp")]
    SnapshotTrackingFailed,

    #[error("Timestamp space exhausted")]
    TimestampExhausted,
}

pub type VersionManagerResult<T> = Result<T, VersionManagerError>;

#[derive(Debug, Clone)]
pub struct VersionManagerConfig {
    pub max_concurrent_reads: u32,
    pub wait_timeout: Duration,
    /// Minimum age of a `Pending` write timestamp before
    /// [`crate::mvcc::VersionManager::reap_expired_write_timestamps`] aborts
    /// it as stale.
    ///
    /// This replaces the former force-advance (`max_frontier_stall`) which was
    /// unreachable and, if configured, could publish uncommitted writes.
    pub write_reap_timeout: Duration,
}

impl Default for VersionManagerConfig {
    fn default() -> Self {
        Self {
            max_concurrent_reads: 1000,
            wait_timeout: Duration::from_secs(5),
            write_reap_timeout: Duration::from_secs(60),
        }
    }
}

impl VersionManagerConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_concurrent_reads(mut self, max: u32) -> Self {
        self.max_concurrent_reads = max;
        self
    }

    pub fn with_write_reap_timeout(mut self, timeout: Duration) -> Self {
        self.write_reap_timeout = timeout;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WriteTimestampState {
    Pending,
    Committed,
    Aborted,
}

/// Observable state of one write-timestamp slot.
///
/// `Vanished` covers every timestamp absent from the slot map: slots of
/// long-committed writes are removed when the read frontier advances over a
/// run of terminal slots, and read-only snapshots never own a write slot at
/// all. Vanished slots carry no pending write, so visibility predicates can
/// trust the plain timestamp comparison for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampSlot {
    Pending,
    Committed,
    Aborted,
    Vanished,
}
