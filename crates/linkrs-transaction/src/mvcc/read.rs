//! Read admission control: slot rate limiting, timeout acquisition,
//! release, and the RAII read timestamp guard.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use linkrs_core::types::Timestamp;

use super::types::{VersionManagerError, VersionManagerResult};
use super::VersionManager;

impl VersionManager {
    pub fn read_timestamp(&self) -> Timestamp {
        self.read_ts.load(Ordering::Acquire)
    }

    pub fn acquire_read_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut guard = self.read_lock.lock();
        loop {
            let pr = self.read_pending.load(Ordering::Relaxed);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_reads as i32 {
                    log::warn!(
                        "Too many pending read requests: {}. Max concurrent reads: {}. \
                        Consider increasing max_concurrent_reads or reducing read intensity.",
                        pr,
                        self.config.max_concurrent_reads,
                    );
                    self.read_condvar.wait(&mut guard);
                    continue;
                }
                self.read_pending.fetch_add(1, Ordering::Relaxed);
                let ts = self.read_ts.load(Ordering::Acquire);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.read_pending.fetch_sub(1, Ordering::Relaxed);
                    self.read_condvar.notify_all();
                    return Err(VersionManagerError::SnapshotTrackingFailed);
                }
                return Ok(ts);
            }
            self.read_condvar.wait(&mut guard);
        }
    }

    pub fn acquire_read_timestamp_with_timeout(&self, timeout: Duration) -> Option<Timestamp> {
        let start = Instant::now();
        let mut guard = self.read_lock.lock();
        loop {
            let pr = self.read_pending.load(Ordering::Relaxed);
            if pr >= 0 {
                if pr >= self.config.max_concurrent_reads as i32 {
                    log::warn!(
                        "Too many pending read requests: {}. Max concurrent reads: {}.",
                        pr,
                        self.config.max_concurrent_reads,
                    );
                    let elapsed = start.elapsed();
                    if elapsed >= timeout {
                        return None;
                    }
                    let remaining = timeout - elapsed;
                    let result = self.read_condvar.wait_for(&mut guard, remaining);
                    if result.timed_out() {
                        return None;
                    }
                    continue;
                }
                self.read_pending.fetch_add(1, Ordering::Relaxed);
                let ts = self.read_ts.load(Ordering::Acquire);
                drop(guard);
                if let Err(e) = self.snapshot_tracker.add_snapshot(ts) {
                    log::error!("Failed to track read snapshot {}: {}", ts, e);
                    self.read_pending.fetch_sub(1, Ordering::Relaxed);
                    return None;
                }
                return Some(ts);
            }

            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return None;
            }

            let remaining = timeout - elapsed;
            let result = self.read_condvar.wait_for(&mut guard, remaining);
            if result.timed_out() {
                return None;
            }
        }
    }

    pub fn release_read_timestamp(&self) {
        let ts = self.read_ts.load(Ordering::Acquire);
        self.release_read_timestamp_at(ts);
    }

    pub fn release_read_timestamp_at(&self, ts: Timestamp) {
        if let Err(e) = self.snapshot_tracker.release_snapshot(ts) {
            log::error!("Failed to release snapshot {}: {}", ts, e);
            // Continue anyway - we still need to decrement read_pending
        }
        self.read_pending.fetch_sub(1, Ordering::Relaxed);
        self.read_condvar.notify_all();
    }
}

pub struct ReadTimestampGuard {
    version_manager: Arc<VersionManager>,
    timestamp: Timestamp,
}

impl ReadTimestampGuard {
    pub fn new(version_manager: Arc<VersionManager>) -> VersionManagerResult<Self> {
        let timestamp = version_manager.acquire_read_timestamp()?;
        Ok(Self {
            version_manager,
            timestamp,
        })
    }

    pub fn timestamp(&self) -> Timestamp {
        self.timestamp
    }
}

impl Drop for ReadTimestampGuard {
    fn drop(&mut self) {
        self.version_manager
            .release_read_timestamp_at(self.timestamp);
    }
}
